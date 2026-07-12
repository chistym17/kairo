use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::context::LlmRequest;
use crate::state::step::Action;

pub mod agentrouter_client;
pub mod deepseek_client;
pub mod groq_client;
pub mod schema;

use crate::config::Config;

use self::agentrouter_client::AgentRouterClient;
use self::deepseek_client::DeepSeekClient;
use self::groq_client::GroqClient;

/// One tool call returned by the LLM, including the provider's tool_call_id.
#[derive(Debug, Clone)]
pub struct LlmToolCall {
    pub action: Action,
    pub tool_call_id: String,
}

/// Build the configured LLM client (`LLM_PROVIDER=groq|deepseek|agentrouter`).
pub fn build_client(config: &Config) -> Box<dyn LlmClient> {
    match config.llm_provider.as_str() {
        "deepseek" => Box::new(DeepSeekClient::new(
            &config.deepseek_api_key,
            &config.model_id,
        )),
        "groq" => Box::new(GroqClient::new(&config.groq_api_key, &config.model_id)),
        "agentrouter" => Box::new(AgentRouterClient::new(
            &config.agentrouter_api_key,
            &config.model_id,
            &config.agentrouter_base_url,
        )),
        other => panic!(
            "unknown LLM_PROVIDER '{}'. Use 'groq', 'deepseek', or 'agentrouter'",
            other
        ),
    }
}

#[async_trait]
pub trait LlmClient: Send + Sync {
    async fn next_action(&self, request: &LlmRequest) -> Result<LlmToolCall, LlmError>;

    fn model_id(&self) -> &str;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LlmError {
    MalformedToolCall(String),

    ApiError(String),

    /// Provider rejected the model's tool call (HTTP 400, code tool_use_failed).
    /// Often transient — the model had the right idea but bad formatting.
    ToolUseFailed(String),

    RateLimited,

    NoToolCall(String),
}

impl std::fmt::Display for LlmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LlmError::MalformedToolCall(msg) => write!(f, "malformed tool call: {}", msg),
            LlmError::ApiError(msg) => write!(f, "API error: {}", msg),
            LlmError::ToolUseFailed(msg) => write!(f, "tool_use_failed: {}", msg),
            LlmError::RateLimited => write!(f, "rate limited"),
            LlmError::NoToolCall(text) => {
                write!(
                    f,
                    "model returned text instead of tool call: {}",
                    &text[..text.len().min(200)]
                )
            }
        }
    }
}
