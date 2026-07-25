use std::time::Instant;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::context::LlmRequest;
use crate::llm::protocol::normalize_openai_chat_choice;
use crate::llm::usage::parse_openai_usage;
use crate::llm::{LlmClient, LlmError, LlmTurn};

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
    async fn respond(&self, request: &LlmRequest) -> Result<LlmTurn, LlmError> {
        let body = json!({
            "model": self.model,
            "messages": request.messages.clone(),
            "tools": request.tools.clone(),
            "tool_choice": "required",
            "max_tokens": 1024,
            "temperature": 0.0
        });

        let started = Instant::now();
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

        openai_turn_from_json(&response_json, started)
    }

    fn model_id(&self) -> &str {
        &self.model
    }
}

pub(crate) fn openai_turn_from_json(
    response_json: &Value,
    started: Instant,
) -> Result<LlmTurn, LlmError> {
    let choice = response_json["choices"]
        .get(0)
        .ok_or_else(|| LlmError::ApiError("response has no choices".to_string()))?;

    let mut usage = parse_openai_usage(&response_json["usage"]);
    usage.total_latency_ms = started.elapsed().as_millis() as u64;
    // TTFT unavailable without streaming.
    usage.time_to_first_token_ms = None;

    Ok(LlmTurn {
        response: normalize_openai_chat_choice(choice),
        usage,
    })
}
