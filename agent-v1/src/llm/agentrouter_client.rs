use async_trait::async_trait;
use serde_json::{json, Value};

use crate::context::LlmRequest;
use crate::llm::protocol::{normalize_openai_chat_choice, AgentResponse};
use crate::llm::{LlmClient, LlmError};

const DEFAULT_BASE_URL: &str = "https://agentrouter.org/v1";

/// Fingerprint matching Qwen Code / OpenAI Node SDK — AgentRouter's WAF
/// rejects generic HTTP clients (e.g. bare reqwest) with unauthorized_client_error.
const USER_AGENT: &str = "QwenCode/0.2.0 (linux; x64)";
const STAINLESS_PACKAGE_VERSION: &str = "6.34.0";

/// OpenAI-compatible AgentRouter chat client.
///
/// AgentRouter is a gateway: same `/v1/chat/completions` shape as OpenAI,
/// with model IDs chosen via `MODEL_ID` (DeepSeek, GLM, Claude, GPT, etc.).
pub struct AgentRouterClient {
    api_key: String,
    model: String,
    api_url: String,
    http: reqwest::Client,
}

impl AgentRouterClient {
    pub fn new(
        api_key: impl Into<String>,
        model: impl Into<String>,
        base_url: impl Into<String>,
    ) -> Self {
        let base = base_url.into().trim_end_matches('/').to_string();
        let api_url = format!("{}/chat/completions", base);
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
            "temperature": 0.0
        });

        let response_json = self.post_json(&body).await?;
        let message = &response_json["choices"].get(0).map(|c| &c["message"]);
        let text = message
            .and_then(|m| {
                m["content"]
                    .as_str()
                    .filter(|s| !s.trim().is_empty())
                    .or_else(|| m["reasoning_content"].as_str())
            })
            .unwrap_or("")
            .trim()
            .to_string();

        if text.is_empty() {
            return Err(LlmError::ApiError(
                "agentrouter ping: empty message content".to_string(),
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
            .header("Accept", "application/json")
            .header("User-Agent", USER_AGENT)
            .header("X-Stainless-Lang", "js")
            .header("X-Stainless-Runtime", "node")
            .header("X-Stainless-Runtime-Version", "node/20.18.0")
            .header("X-Stainless-Package-Version", STAINLESS_PACKAGE_VERSION)
            .header("X-Stainless-OS", "Linux")
            .header("X-Stainless-Arch", "x64")
            .header("X-Stainless-Retry-Count", "0")
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

        let text = response
            .text()
            .await
            .map_err(|e| LlmError::ApiError(format!("failed to read response body: {}", e)))?;

        // Some AgentRouter upstreams double-encode JSON as a string; unwrap once.
        let parsed: Value = serde_json::from_str(&text).map_err(|e| {
            LlmError::ApiError(format!("failed to parse response JSON: {} — body: {}", e, text))
        })?;

        if let Some(s) = parsed.as_str() {
            serde_json::from_str(s).map_err(|e| {
                LlmError::ApiError(format!(
                    "failed to parse double-encoded response JSON: {}",
                    e
                ))
            })
        } else {
            Ok(parsed)
        }
    }
}

#[async_trait]
impl LlmClient for AgentRouterClient {
    async fn respond(&self, request: &LlmRequest) -> Result<AgentResponse, LlmError> {
        let body = json!({
            "model": self.model,
            "messages": request.messages.clone(),
            "tools": request.tools.clone(),
            "tool_choice": "required",
            "max_tokens": 2048,
            "temperature": 0.0
        });

        let response_json = self.post_json(&body).await?;

        let choice = response_json["choices"]
            .get(0)
            .ok_or_else(|| LlmError::ApiError("response has no choices".to_string()))?;

        Ok(normalize_openai_chat_choice(choice))
    }

    fn model_id(&self) -> &str {
        &self.model
    }
}

pub fn default_base_url() -> String {
    DEFAULT_BASE_URL.to_string()
}
