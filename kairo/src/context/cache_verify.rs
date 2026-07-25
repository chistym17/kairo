//! Cache verification helpers (Phase 13A Day 5).
//!
//! Separates **harness prefix churn** from **provider cache behavior**.

use serde::{Deserialize, Serialize};

use super::prompt_layout::PromptDiagnostics;

/// Why consecutive stable prefixes likely diverged (local diagnosis).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PrefixChangeReason {
    Unchanged,
    ToolsChanged,
    SystemChanged,
    InstructionsChanged,
    SessionChanged,
    /// Multiple components differ, or only overall hash differs.
    MultipleOrUnknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PrefixCompare {
    pub previous_stable_hash: String,
    pub current_stable_hash: String,
    pub reason: PrefixChangeReason,
    pub tools_changed: bool,
    pub system_changed: bool,
    pub instructions_changed: bool,
    pub session_changed: bool,
}

impl PrefixCompare {
    pub fn compare(prev: &PromptDiagnostics, curr: &PromptDiagnostics) -> Self {
        let tools_changed = prev.tool_schema_hash != curr.tool_schema_hash;
        let system_changed = prev.system_prompt_hash != curr.system_prompt_hash;
        let instructions_changed =
            prev.project_instructions_hash != curr.project_instructions_hash;
        let session_changed = prev.session_messages_hash != curr.session_messages_hash;
        let stable_same = prev.stable_prefix_hash == curr.stable_prefix_hash;

        let reason = if stable_same {
            PrefixChangeReason::Unchanged
        } else {
            let count = [
                tools_changed,
                system_changed,
                instructions_changed,
                session_changed,
            ]
            .into_iter()
            .filter(|x| *x)
            .count();
            match (
                count,
                tools_changed,
                system_changed,
                instructions_changed,
                session_changed,
            ) {
                (1, true, _, _, _) => PrefixChangeReason::ToolsChanged,
                (1, _, true, _, _) => PrefixChangeReason::SystemChanged,
                (1, _, _, true, _) => PrefixChangeReason::InstructionsChanged,
                (1, _, _, _, true) => PrefixChangeReason::SessionChanged,
                _ => PrefixChangeReason::MultipleOrUnknown,
            }
        };

        Self {
            previous_stable_hash: prev.stable_prefix_hash.clone(),
            current_stable_hash: curr.stable_prefix_hash.clone(),
            reason,
            tools_changed,
            system_changed,
            instructions_changed,
            session_changed,
        }
    }

    pub fn render_compact(&self) -> String {
        format!(
            "prefix={:?} tools={} system={} instructions={} session={}",
            self.reason,
            self.tools_changed,
            self.system_changed,
            self.instructions_changed,
            self.session_changed
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authorization::EditPolicy;
    use crate::context::prompt_layout::build_prompt_layout;
    use crate::context::{default_system_prompt, ContextBuilder};
    use crate::llm::schema::build_tools_schema;
    use crate::session::{Session, SessionSettings};
    use serde_json::json;

    /// Matrix A (local): consecutive goals in one session keep the same stable prefix.
    #[test]
    fn matrix_a_consecutive_goals_same_stable_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let session = [
            crate::session::Message::user("prior"),
            crate::session::Message::assistant("ok"),
        ];
        let a = ContextBuilder::new().build(dir.path(), "turn 1", &session, &[]);
        let b = ContextBuilder::new().build(dir.path(), "turn 2", &session, &[]);
        let cmp = PrefixCompare::compare(&a.diagnostics, &b.diagnostics);
        assert_eq!(cmp.reason, PrefixChangeReason::Unchanged);
        assert_eq!(
            a.diagnostics.stable_prefix_hash,
            b.diagnostics.stable_prefix_hash
        );
    }

    /// Matrix C (local): appended policy update grows session; tools/system unchanged;
    /// earlier messages remain byte-identical.
    #[test]
    fn matrix_c_appended_policy_preserves_earlier_and_tools() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = Session::new(
            dir.path(),
            SessionSettings {
                edit_policy: EditPolicy::ConfirmBeforeFirstEdit,
                ..SessionSettings::default()
            },
        );
        session.add_user_message("hello");
        session.add_assistant_message("hi");
        let earlier = session.messages.clone();

        let before = ContextBuilder::new().build(dir.path(), "goal", &session.messages, &[]);
        session.set_edit_policy(EditPolicy::Auto);
        assert_eq!(&session.messages[..earlier.len()], earlier.as_slice());

        let after = ContextBuilder::new().build(dir.path(), "goal", &session.messages, &[]);
        let cmp = PrefixCompare::compare(&before.diagnostics, &after.diagnostics);
        assert_eq!(cmp.reason, PrefixChangeReason::SessionChanged);
        assert!(!cmp.tools_changed);
        assert!(!cmp.system_changed);
        assert_ne!(
            before.diagnostics.stable_prefix_hash,
            after.diagnostics.stable_prefix_hash
        );
        assert_eq!(
            before.diagnostics.tool_schema_hash,
            after.diagnostics.tool_schema_hash
        );
    }

    /// Matrix D (local): tool schema change flips tools hash / stable prefix.
    #[test]
    fn matrix_d_tool_schema_change_misses_from_tools() {
        let dir = tempfile::tempdir().unwrap();
        let a = ContextBuilder::new().build(dir.path(), "goal", &[], &[]);

        let base = default_system_prompt();
        let mut tools = build_tools_schema();
        if let Some(arr) = tools.as_array_mut() {
            if let Some(t) = arr.first_mut() {
                t["function"]["description"] = json!("matrix-d-changed");
            }
        }
        let layout = build_prompt_layout(&base, None, &base, tools, &[], "goal", None, &[]);
        let b_diag = layout.diagnostics();
        let cmp = PrefixCompare::compare(&a.diagnostics, &b_diag);
        assert_eq!(cmp.reason, PrefixChangeReason::ToolsChanged);
        assert!(cmp.tools_changed);
        assert!(!cmp.system_changed);
        assert!(!cmp.session_changed);
    }
}
