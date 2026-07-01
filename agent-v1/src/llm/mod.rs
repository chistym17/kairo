use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::state::step::{Action, Step};

pub mod groq_client;
pub mod schema;

#[async_trait]
pub trait LlmClient: Send + Sync {
    async fn next_action(
        &self,
        system_prompt: &str,
        goal: &str,
        history: &[Step],
    ) -> Result<Action, LlmError>;

    fn model_id(&self) -> &str;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LlmError {
    MalformedToolCall(String),

    ApiError(String),

    RateLimited,

    NoToolCall(String),
}

impl std::fmt::Display for LlmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LlmError::MalformedToolCall(msg) => write!(f, "malformed tool call: {}", msg),
            LlmError::ApiError(msg) => write!(f, "API error: {}", msg),
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
