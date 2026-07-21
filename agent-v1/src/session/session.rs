use std::path::PathBuf;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::authorization::{AuthorizationState, EditPolicy};

use super::message::Message;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionSettings {
    pub model_id: String,
    pub max_steps: u32,
    /// Initial edit policy for new sessions (copied into [`AuthorizationState`]).
    #[serde(default)]
    pub edit_policy: EditPolicy,
}

impl Default for SessionSettings {
    fn default() -> Self {
        Self {
            model_id: "llama-3.1-70b-versatile".to_string(),
            max_steps: 15,
            edit_policy: EditPolicy::Auto,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompactionState {
    /// Number of oldest session messages represented by `summary`.
    pub summarized_until: usize,

    /// Deterministic summary of session history up to `summarized_until`.
    pub summary: String,
}

/// Long-lived agent session for a single workspace.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Session {
    pub id: String,
    pub workspace_root: PathBuf,
    pub messages: Vec<Message>,
    pub compaction: Option<CompactionState>,
    pub settings: SessionSettings,
    /// Edit policy + task-scoped mutation grants (Phase 12).
    #[serde(default)]
    pub authorization: AuthorizationState,
    pub created_at: String,
    pub updated_at: String,
}

impl Session {
    pub fn new(workspace_root: impl Into<PathBuf>, settings: SessionSettings) -> Self {
        let now = Utc::now().to_rfc3339();
        let authorization = AuthorizationState::new(settings.edit_policy);
        Self {
            id: Uuid::new_v4().to_string(),
            workspace_root: workspace_root.into(),
            messages: Vec::new(),
            compaction: None,
            settings,
            authorization,
            created_at: now.clone(),
            updated_at: now,
        }
    }

    pub fn add_message(&mut self, message: Message) {
        self.messages.push(message);
        self.touch();
    }

    pub fn add_user_message(&mut self, content: impl Into<String>) {
        self.add_message(Message::user(content));
    }

    pub fn add_assistant_message(&mut self, content: impl Into<String>) {
        self.add_message(Message::assistant(content));
    }

    pub fn add_tool_message(&mut self, name: impl Into<String>, result: impl Into<String>) {
        self.add_message(Message::tool(name, result));
    }

    fn touch(&mut self) {
        self.updated_at = Utc::now().to_rfc3339();
    }
}
