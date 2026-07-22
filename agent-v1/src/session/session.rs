use std::path::PathBuf;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::authorization::{AuthorizationState, EditPolicy};

use super::intent::TurnIntent;
use super::message::Message;
use super::task_state::TaskSessionState;

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
    /// Bump when on-disk session JSON shape changes incompatibly.
    #[serde(default = "current_schema_version")]
    pub schema_version: u32,
    pub id: String,
    pub workspace_root: PathBuf,
    pub messages: Vec<Message>,
    pub compaction: Option<CompactionState>,
    pub settings: SessionSettings,
    /// Edit policy + task-scoped mutation grants (Phase 12).
    #[serde(default)]
    pub authorization: AuthorizationState,
    /// Compact durable task memory (plan, decisions, files, verification).
    #[serde(default)]
    pub task: TaskSessionState,
    /// Last declared / inferred turn intent (metadata only).
    #[serde(default)]
    pub declared_intent: TurnIntent,
    /// When true, [`declared_intent`] was set via `/intent` and is not overwritten by inference.
    #[serde(default)]
    pub intent_pinned: bool,
    pub created_at: String,
    pub updated_at: String,
}

/// Current on-disk session schema. Older files migrate forward on load.
pub const SESSION_SCHEMA_VERSION: u32 = 1;

fn current_schema_version() -> u32 {
    SESSION_SCHEMA_VERSION
}

impl Session {
    pub fn new(workspace_root: impl Into<PathBuf>, settings: SessionSettings) -> Self {
        let now = Utc::now().to_rfc3339();
        let authorization = AuthorizationState::new(settings.edit_policy);
        Self {
            schema_version: SESSION_SCHEMA_VERSION,
            id: Uuid::new_v4().to_string(),
            workspace_root: workspace_root.into(),
            messages: Vec::new(),
            compaction: None,
            settings,
            authorization,
            task: TaskSessionState::default(),
            declared_intent: TurnIntent::Explore,
            intent_pinned: false,
            created_at: now.clone(),
            updated_at: now,
        }
    }

    pub fn set_intent(&mut self, intent: TurnIntent) {
        self.declared_intent = intent;
        self.intent_pinned = true;
        self.touch();
    }

    pub fn clear_intent_pin(&mut self) {
        self.intent_pinned = false;
        self.touch();
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
