use async_trait::async_trait;
use serde_json::{json, Value};

use crate::llm::schema::{build_tools_schema, parse_tool_call};
use crate::llm::{LlmClient, LlmError};
use crate::state::step::{Action, Step};
use crate::tools::ToolResult;

const GROQ_API_URL: &str = "https://api.groq.com/openai/v1/chat/completions";

pub struct GroqClient {
    api_key: String,
    model: String,
    http: reqwest::Client,
}

impl GroqClient {
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            model: model.into(),
            http: reqwest::Client::new(),
        }
    }
    fn build_messages(&self, system_prompt: &str, goal: &str, history: &[Step]) -> Value {
        let mut messages = vec![
            json!({
                "role": "system",
                "content": system_prompt
            }),
            json!({
                "role": "user",
                "content": goal
            }),
        ];

        for step in history {
            let tool_call_msg = action_to_assistant_message(&step.action);
            messages.push(tool_call_msg);

            let tool_result_msg = tool_result_to_message(&step.action, &step.result);
            messages.push(tool_result_msg);
        }

        json!(messages)
    }
}

#[async_trait]
impl LlmClient for GroqClient {
    async fn next_action(
        &self,
        system_prompt: &str,
        goal: &str,
        history: &[Step],
    ) -> Result<Action, LlmError> {
        let messages = self.build_messages(system_prompt, goal, history);
        let tools = build_tools_schema();

        let body = json!({
            "model": self.model,
            "messages": messages,
            "tools": tools,
            "tool_choice": "required",
            "max_tokens": 1024,
            "temperature": 0.0  // deterministic — important for reproducible benchmark runs
        });

        let response = self
            .http
            .post(GROQ_API_URL)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| LlmError::ApiError(format!("request failed: {}", e)))?;

        let status = response.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(LlmError::RateLimited);
        }

        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(LlmError::ApiError(format!("HTTP {}: {}", status, body)));
        }

        let response_json: Value = response
            .json()
            .await
            .map_err(|e| LlmError::ApiError(format!("failed to parse response JSON: {}", e)))?;

        let choice = response_json["choices"]
            .get(0)
            .ok_or_else(|| LlmError::ApiError("response has no choices".to_string()))?;

        let message = &choice["message"];
        let finish_reason = choice["finish_reason"].as_str().unwrap_or("");

        if finish_reason == "tool_calls" || message["tool_calls"].is_array() {
            let tool_calls = message["tool_calls"].as_array().ok_or_else(|| {
                LlmError::MalformedToolCall("tool_calls is not an array".to_string())
            })?;

            let tool_call = tool_calls.first().ok_or_else(|| {
                LlmError::MalformedToolCall("tool_calls array is empty".to_string())
            })?;

            let tool_name = tool_call["function"]["name"]
                .as_str()
                .ok_or_else(|| LlmError::MalformedToolCall("missing function.name".to_string()))?;

            let args_str = tool_call["function"]["arguments"].as_str().ok_or_else(|| {
                LlmError::MalformedToolCall("missing function.arguments".to_string())
            })?;

            let args: Value = serde_json::from_str(args_str).map_err(|e| {
                LlmError::MalformedToolCall(format!(
                    "arguments is not valid JSON: {} — raw: {}",
                    e, args_str
                ))
            })?;

            parse_tool_call(tool_name, &args).map_err(|e| LlmError::MalformedToolCall(e))
        } else {
            let text = message["content"]
                .as_str()
                .unwrap_or("[no content]")
                .to_string();

            Err(LlmError::NoToolCall(text))
        }
    }

    fn model_id(&self) -> &str {
        &self.model
    }
}

fn action_to_assistant_message(action: &Action) -> Value {
    let (tool_name, args) = action_to_tool_call_parts(action);

    json!({
        "role": "assistant",
        "content": null,
        "tool_calls": [{
            "id": format!("call_{}", uuid::Uuid::new_v4().to_string().replace("-", "")[..8].to_string()),
            "type": "function",
            "function": {
                "name": tool_name,
                "arguments": serde_json::to_string(&args).unwrap_or_default()
            }
        }]
    })
}

fn tool_result_to_message(action: &Action, result: &ToolResult) -> Value {
    let (tool_name, _) = action_to_tool_call_parts(action);

    let content = if result.success {
        let mut parts = vec![result.output.clone()];
        if let Some(diff) = &result.diff {
            if !diff.is_empty() {
                parts.push(format!("\nDiff:\n{}", diff));
            }
        }
        parts.join("")
    } else {
        let mut parts = vec![];
        if !result.output.is_empty() {
            parts.push(result.output.clone());
        }
        if let Some(err) = &result.error {
            parts.push(format!("Error: {}", err));
        }
        if parts.is_empty() {
            "Tool call failed with no output.".to_string()
        } else {
            parts.join("\n")
        }
    };

    json!({
        "role": "tool",
        "tool_call_id": format!("call_{}", &tool_name[..4.min(tool_name.len())]),
        "content": content
    })
}

fn action_to_tool_call_parts(action: &Action) -> (&'static str, Value) {
    match action {
        Action::ReadFile { path } => ("read_file", json!({ "path": path.to_string_lossy() })),
        Action::WriteFile { path, content } => (
            "write_file",
            json!({
                "path": path.to_string_lossy(),
                "content": content
            }),
        ),
        Action::ApplyPatch { path, diff } => (
            "apply_patch",
            json!({
                "path": path.to_string_lossy(),
                "diff": diff
            }),
        ),
        Action::RunCommand { cmd } => ("run_command", json!({ "cmd": cmd })),
        Action::Finish { summary } => ("finish", json!({ "summary": summary })),
    }
}
