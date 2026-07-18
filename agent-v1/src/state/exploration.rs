use crate::state::step::Action;

/// Consecutive single read-only turns before injecting a batching nudge.
pub const SERIAL_READONLY_NUDGE_AFTER: u32 = 3;

/// Tracks serial exploration for soft batching guidance only.
/// Does not gate tools or infer readiness to edit.
#[derive(Debug, Default, Clone)]
pub struct ExplorationTracker {
    /// Streak of model turns that each had exactly one read-only tool.
    pub consecutive_single_readonly_turns: u32,
    /// Model turns since the last successful write/patch (or since run start).
    pub model_turns_since_edit: u32,
    /// Turns whose tools were all exploration/read-only.
    pub exploration_turns: u32,
    /// Sum of tool calls in those exploration turns (for avg).
    pub readonly_tools_in_exploration: u32,
    pub serial_batch_nudges: u32,
    /// Retained for metrics compatibility; no longer escalated.
    pub no_edit_nudges: u32,
    /// Avoid double-counting the same nudge across LLM retries.
    last_nudge_key: Option<u32>,
}

impl ExplorationTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn observe_turn(&mut self, actions: &[Action]) {
        if actions.is_empty() {
            return;
        }

        self.model_turns_since_edit = self.model_turns_since_edit.saturating_add(1);

        let all_readonly = actions.iter().all(is_exploration_readonly);
        let n = actions.len() as u32;

        if all_readonly {
            self.exploration_turns += 1;
            self.readonly_tools_in_exploration += n;
            if n == 1 {
                self.consecutive_single_readonly_turns += 1;
            } else {
                self.consecutive_single_readonly_turns = 0;
            }
        } else {
            self.consecutive_single_readonly_turns = 0;
        }
    }

    pub fn note_edit(&mut self) {
        self.model_turns_since_edit = 0;
        self.consecutive_single_readonly_turns = 0;
        self.last_nudge_key = None;
    }

    pub fn avg_readonly_tools_per_exploration_turn(&self) -> Option<f64> {
        if self.exploration_turns == 0 {
            None
        } else {
            Some(self.readonly_tools_in_exploration as f64 / self.exploration_turns as f64)
        }
    }

    /// Kept for metrics / optional semantic re-enable; always 0 when unused.
    pub fn no_edit_escalation_level(&self) -> u32 {
        0
    }

    /// Soft guidance for the next LLM request (does not block tools).
    pub fn nudge_for_injection(&mut self) -> Option<String> {
        if self.consecutive_single_readonly_turns < SERIAL_READONLY_NUDGE_AFTER {
            return None;
        }

        let key = self.consecutive_single_readonly_turns;
        let text = format!(
            "You are exploring inefficiently ({} single read-only turns in a row). \
             In your next response, batch all currently known independent searches and reads \
             in one turn (multiple tool calls).",
            self.consecutive_single_readonly_turns
        );

        if self.last_nudge_key != Some(key) {
            self.last_nudge_key = Some(key);
            self.serial_batch_nudges += 1;
        }

        Some(text)
    }
}

pub fn is_exploration_readonly(action: &Action) -> bool {
    matches!(
        action,
        Action::ReadFile { .. }
            | Action::ListFiles { .. }
            | Action::SearchCode { .. }
            | Action::Locate { .. }
            | Action::FindFiles { .. }
            | Action::DetectProjectSnapshot { .. }
            | Action::GitStatus
            | Action::GitDiff { .. }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn read() -> Action {
        Action::ReadFile {
            path: PathBuf::from("a.rs"),
            offset: 1,
            limit: 50,
        }
    }

    fn search() -> Action {
        Action::SearchCode {
            query: "foo".into(),
            path: PathBuf::from("."),
        }
    }

    #[test]
    fn serial_nudge_after_three_single_reads() {
        let mut t = ExplorationTracker::new();
        t.observe_turn(&[read()]);
        t.observe_turn(&[search()]);
        assert!(t.nudge_for_injection().is_none());
        t.observe_turn(&[read()]);
        let text = t.nudge_for_injection().unwrap();
        assert!(text.contains("exploring inefficiently"));
        assert_eq!(t.serial_batch_nudges, 1);
        let _ = t.nudge_for_injection();
        assert_eq!(t.serial_batch_nudges, 1);
    }

    #[test]
    fn no_edit_escalation_disabled() {
        let mut t = ExplorationTracker::new();
        for _ in 0..20 {
            t.observe_turn(&[read()]);
        }
        assert_eq!(t.no_edit_escalation_level(), 0);
        assert_eq!(t.no_edit_nudges, 0);
    }
}
