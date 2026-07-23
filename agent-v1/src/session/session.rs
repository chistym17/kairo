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
        let prev = self.declared_intent;
        self.declared_intent = intent;
        self.intent_pinned = true;
        if prev != intent {
            self.append_session_update(format!(
                "Turn intent pinned to {} (was {}).",
                intent.as_str(),
                prev.as_str()
            ));
        } else {
            self.append_session_update(format!(
                "Turn intent pinned to {}.",
                intent.as_str()
            ));
        }
    }

    pub fn clear_intent_pin(&mut self) {
        if !self.intent_pinned {
            return;
        }
        self.intent_pinned = false;
        self.append_session_update("Turn intent pin cleared (resume inference).");
    }

    /// Append-only session note for provider prompt-cache stability (Phase 13A Day 4).
    ///
    /// Never rewrite earlier messages when policy / plan / approvals change — append here.
    pub fn append_session_update(&mut self, body: impl AsRef<str>) {
        let body = body.as_ref().trim();
        if body.is_empty() {
            return;
        }
        self.add_assistant_message(format!("Session update:\n{body}"));
    }

    /// Change edit policy and append a session update (does not rewrite history).
    pub fn set_edit_policy(&mut self, policy: EditPolicy) {
        let from = self.authorization.edit_policy;
        if from == policy {
            return;
        }
        self.authorization.edit_policy = policy;
        self.settings.edit_policy = policy;
        self.append_session_update(format!(
            "EditPolicy changed from {} to {}.",
            from.as_str(),
            policy.as_str()
        ));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_session_update_preserves_earlier_messages() {
        let mut session = Session::new("/tmp/ws", SessionSettings::default());
        session.add_user_message("first");
        session.add_assistant_message("reply");
        let before = session.messages.clone();

        session.append_session_update("EditPolicy changed to auto.");
        assert_eq!(session.messages.len(), before.len() + 1);
        assert_eq!(&session.messages[..before.len()], before.as_slice());
        match session.messages.last() {
            Some(Message::Assistant { content }) => {
                assert!(content.starts_with("Session update:"));
                assert!(content.contains("EditPolicy changed to auto."));
            }
            other => panic!("expected assistant update, got {other:?}"),
        }
    }

    #[test]
    fn set_edit_policy_appends_without_rewriting() {
        let mut session = Session::new(
            "/tmp/ws",
            SessionSettings {
                edit_policy: EditPolicy::ConfirmBeforeFirstEdit,
                ..SessionSettings::default()
            },
        );
        session.add_user_message("hello");
        let prefix = session.messages.clone();

        session.set_edit_policy(EditPolicy::Auto);
        assert_eq!(session.authorization.edit_policy, EditPolicy::Auto);
        assert_eq!(&session.messages[..prefix.len()], prefix.as_slice());
        assert!(matches!(
            session.messages.last(),
            Some(Message::Assistant { content }) if content.contains("EditPolicy changed")
                && content.contains("confirm")
                && content.contains("auto")
        ));

        let len = session.messages.len();
        session.set_edit_policy(EditPolicy::Auto); // no-op
        assert_eq!(session.messages.len(), len);
    }
}

