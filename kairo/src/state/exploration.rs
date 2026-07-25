use crate::state::step::Action;

/// Consecutive single read-only turns before injecting a productive-action nudge.
pub const SERIAL_READONLY_NUDGE_AFTER: u32 = 3;

/// After this many model turns without an edit, push plan / patch / finish instead
/// of more exploration (soft guidance only).
pub const NO_EDIT_STOP_EXPLORE_AFTER: u32 = 8;

/// Tracks serial exploration for soft guidance only.
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
    /// Soft no-edit “stop exploring” nudges (not semantic tool gating).
    pub no_edit_nudges: u32,
    /// Avoid double-counting the same nudge across LLM retries.
    last_nudge_key: Option<(u32, u32)>,
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

    /// Kept for metrics / optional semantic re-enable.
    pub fn no_edit_escalation_level(&self) -> u32 {
        if self.model_turns_since_edit >= NO_EDIT_STOP_EXPLORE_AFTER {
            1
        } else {
            0
        }
    }

    /// Soft guidance for the next LLM request (does not block tools).
    pub fn nudge_for_injection(&mut self) -> Option<String> {
        // Prefer "stop exploring" over serial thrash once the run is stuck.
        if self.model_turns_since_edit >= NO_EDIT_STOP_EXPLORE_AFTER {
            let key = (1u32, self.model_turns_since_edit);
            let text = format!(
                "You have explored for {} model turns without editing. Stop re-reading \
                 already-supplied ranges (duplicate full reads are denied). Next response \
                 must do one of: update_plan with a concrete checklist, apply_patch/write_file \
                 for the fix, ask a focused question, or finish with a blocker naming the \
                 exact missing fact.",
                self.model_turns_since_edit
            );
            if self.last_nudge_key != Some(key) {
                self.last_nudge_key = Some(key);
                self.no_edit_nudges = self.no_edit_nudges.saturating_add(1);
            }
            return Some(text);
        }

        if self.consecutive_single_readonly_turns < SERIAL_READONLY_NUDGE_AFTER {
            return None;
        }

        let key = (0u32, self.consecutive_single_readonly_turns);
        let text = format!(
            "You are exploring inefficiently ({} single read-only turns in a row). \
             Next response: take a different productive action — locate a new contract \
             boundary, update_plan, propose/apply a patch, ask the user, or finish with \
             findings. Only batch additional reads if they target unseen evidence.",
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
            | Action::SummarizeRepo { .. }
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
        assert!(text.contains("different productive action"));
        assert_eq!(t.serial_batch_nudges, 1);
        let _ = t.nudge_for_injection();
        assert_eq!(t.serial_batch_nudges, 1);
    }

    #[test]
    fn no_edit_escalation_after_threshold() {
        let mut t = ExplorationTracker::new();
        for _ in 0..NO_EDIT_STOP_EXPLORE_AFTER {
            t.observe_turn(&[read()]);
        }
        assert_eq!(t.no_edit_escalation_level(), 1);
        let text = t.nudge_for_injection().unwrap();
        assert!(text.contains("without editing"));
        assert_eq!(t.no_edit_nudges, 1);
    }
}
