use crate::state::step::{Action, Step};
use crate::tools::ToolResult;

/// Consecutive single read-only turns before injecting a batching nudge.
pub const SERIAL_READONLY_NUDGE_AFTER: u32 = 3;

use crate::governance::NO_EDIT_NUDGE_THRESHOLDS;

/// Tracks serial exploration and time-to-edit soft budgets.
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

    pub fn no_edit_escalation_level(&self) -> u32 {
        NO_EDIT_NUDGE_THRESHOLDS
            .iter()
            .filter(|&&threshold| self.model_turns_since_edit >= threshold)
            .count() as u32
    }

    /// Soft guidance for the next LLM request (does not block tools).
    /// Dedupes counting when the same nudge is re-injected on API retries.
    pub fn nudge_for_injection(&mut self) -> Option<String> {
        let serial = self.consecutive_single_readonly_turns >= SERIAL_READONLY_NUDGE_AFTER;
        let level = self.no_edit_escalation_level();
        if !serial && level == 0 {
            return None;
        }

        let key = (
            if serial {
                self.consecutive_single_readonly_turns
            } else {
                0
            },
            if level > 0 {
                level
            } else {
                0
            },
        );

        let mut parts = Vec::new();
        if serial {
            parts.push(format!(
                "You are exploring inefficiently ({} single read-only turns in a row). \
                 In your next response, batch all currently known independent searches and reads \
                 in one turn (multiple tool calls).",
                self.consecutive_single_readonly_turns
            ));
        }
        if level > 0 {
            let msg = match level {
                1 => format!(
                    "No file edit in the last {} model turns. Summarize findings and either \
                     patch now or state the one missing fact blocking an edit.",
                    self.model_turns_since_edit
                ),
                2 => format!(
                    "Still no edit after {} turns. Call update_plan if needed, then \
                     apply_patch/write_file or explain the single blocker.",
                    self.model_turns_since_edit
                ),
                3 => "Hard warning: exploration budget nearly exhausted. Next response must \
                      patch, cite one specific missing fact, or finish with a blocker."
                    .to_string(),
                _ => "Tool restriction imminent: patch, one justified new read, update_plan, \
                      or finish/blocker — broad exploration will be blocked next turn."
                    .to_string(),
            };
            parts.push(msg);
        }

        if self.last_nudge_key != Some(key) {
            self.last_nudge_key = Some(key);
            if serial {
                self.serial_batch_nudges += 1;
            }
            if level > 0 {
                self.no_edit_nudges = self.no_edit_nudges.saturating_add(1).min(4);
            }
        }

        Some(parts.join("\n\n"))
    }
}

pub fn is_exploration_readonly(action: &Action) -> bool {
    matches!(
        action,
        Action::ReadFile { .. }
            | Action::ListFiles { .. }
            | Action::SearchCode { .. }
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

    fn patch() -> Action {
        Action::ApplyPatch {
            path: PathBuf::from("a.rs"),
            diff: String::new(),
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
    fn no_edit_escalation_ladder() {
        let mut t = ExplorationTracker::new();
        for _ in 0..NO_EDIT_NUDGE_THRESHOLDS[0] {
            t.observe_turn(&[read()]);
        }
        let text = t.nudge_for_injection().unwrap();
        assert!(text.contains("No file edit") || text.contains("Summarize"));
        assert_eq!(t.no_edit_escalation_level(), 1);

        for _ in 0..(NO_EDIT_NUDGE_THRESHOLDS[3] - NO_EDIT_NUDGE_THRESHOLDS[0]) {
            t.observe_turn(&[read()]);
        }
        assert_eq!(t.no_edit_escalation_level(), 4);
        t.note_edit();
        assert_eq!(t.no_edit_escalation_level(), 0);
        let _ = patch();
    }
}
