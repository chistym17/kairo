use async_trait::async_trait;
use serde_json::{json, Value};

use crate::context::LlmRequest;
use crate::llm::schema::parse_tool_call;
use crate::llm::{LlmClient, LlmError, LlmToolCall};

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
}

#[async_trait]
impl LlmClient for GroqClient {
    async fn next_action(&self, request: &LlmRequest) -> Result<LlmToolCall, LlmError> {
        let body = json!({
            "model": self.model,
            "messages": request.messages.clone(),
            "tools": request.tools.clone(),
            "tool_choice": "required",
            "max_tokens": 1024,
            "temperature": 0.0
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
            if body.contains("tool_use_failed") {
                return Err(LlmError::ToolUseFailed(format!("HTTP {}: {}", status, body)));
            }
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

            let tool_call_id = tool_call["id"]
                .as_str()
                .unwrap_or("call_missing")
                .to_string();

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

            let action = parse_tool_call(tool_name, &args).map_err(LlmError::MalformedToolCall)?;
            Ok(LlmToolCall {
                action,
                tool_call_id,
            })
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
