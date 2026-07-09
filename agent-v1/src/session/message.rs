use serde::{Deserialize, Serialize};

/// Conversation-level message stored in a session.
///
/// This is not the same as `state::step::Step`, which records one tool
/// execution inside a single agent run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum Message {
    User { content: String },
    Assistant { content: String },
    Tool { name: String, result: String },
}

impl Message {
    pub fn user(content: impl Into<String>) -> Self {
        Self::User {
            content: content.into(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self::Assistant {
            content: content.into(),
        }
    }

    pub fn tool(name: impl Into<String>, result: impl Into<String>) -> Self {
        Self::Tool {
            name: name.into(),
            result: result.into(),
        }
    }
}
