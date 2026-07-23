use async_trait::async_trait;
use serde_json::{json, Value};

use crate::context::LlmRequest;
use crate::llm::groq_client::openai_turn_from_json;
use crate::llm::{LlmClient, LlmError, LlmTurn};

const DEFAULT_DEEPSEEK_API_URL: &str = "https://api.deepseek.com/chat/completions";

/// OpenAI-compatible DeepSeek chat client.
///
/// Notes for V4 Flash/Pro:
/// - defaults to thinking mode, which rejects `tool_choice: "required"`
/// - we use `tool_choice: "auto"` and disable thinking for reliable tool loops
pub struct DeepSeekClient {
    api_key: String,
    model: String,
    api_url: String,
    http: reqwest::Client,
}

impl DeepSeekClient {
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        let api_url = std::env::var("DEEPSEEK_API_URL")
            .unwrap_or_else(|_| DEFAULT_DEEPSEEK_API_URL.to_string());
        Self {
            api_key: api_key.into(),
            model: model.into(),
            api_url,
            http: reqwest::Client::new(),
        }
    }

    /// Minimal connectivity check: one short chat completion (no tools).
    pub async fn ping(&self) -> Result<String, LlmError> {
        let body = json!({
            "model": self.model,
            "messages": [
                {"role": "user", "content": "Reply with exactly: ok"}
            ],
            "max_tokens": 16,
            "temperature": 0.0,
            "thinking": {"type": "disabled"}
        });

        let response_json = self.post_json(&body).await?;
        let text = response_json["choices"]
            .get(0)
            .and_then(|c| c["message"]["content"].as_str())
            .unwrap_or("")
            .trim()
            .to_string();

        if text.is_empty() {
            return Err(LlmError::ApiError(
                "deepseek ping: empty message content".to_string(),
            ));
        }
        Ok(text)
    }

    async fn post_json(&self, body: &Value) -> Result<Value, LlmError> {
        let response = self
            .http
            .post(&self.api_url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(body)
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

        response
            .json()
            .await
            .map_err(|e| LlmError::ApiError(format!("failed to parse response JSON: {}", e)))
    }
}

#[async_trait]
impl LlmClient for DeepSeekClient {
    async fn respond(&self, request: &LlmRequest) -> Result<LlmTurn, LlmError> {
        // V4 thinking mode rejects tool_choice=required; auto + thinking disabled
        // keeps agent tool loops reliable.
        let body = json!({
            "model": self.model,
            "messages": request.messages.clone(),
            "tools": request.tools.clone(),
            "tool_choice": "auto",
            "max_tokens": 2048,
            "temperature": 0.0,
            "thinking": {"type": "disabled"}
        });

        let started = std::time::Instant::now();
        let response_json = self.post_json(&body).await?;
        openai_turn_from_json(&response_json, started)
    }

    fn model_id(&self) -> &str {
        &self.model
    }
}
