use crate::state::step::Action;

/// Consecutive single read-only turns before injecting a batching nudge.
pub const SERIAL_READONLY_NUDGE_AFTER: u32 = 3;
/// Model turns without WriteFile/ApplyPatch before soft "edit or explain" nudge.
pub const NO_EDIT_BUDGET_TURNS: u32 = 8;

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

    /// Soft guidance for the next LLM request (does not block tools).
    /// Dedupes counting when the same nudge is re-injected on API retries.
    pub fn nudge_for_injection(&mut self) -> Option<String> {
        let serial = self.consecutive_single_readonly_turns >= SERIAL_READONLY_NUDGE_AFTER;
        let no_edit = self.model_turns_since_edit >= NO_EDIT_BUDGET_TURNS;
        if !serial && !no_edit {
            return None;
        }

        let key = (
            if serial {
                self.consecutive_single_readonly_turns
            } else {
                0
            },
            if no_edit {
                self.model_turns_since_edit
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
        if no_edit {
            parts.push(format!(
                "No file edit in the last {} model turns. In your next response: \
                 (1) briefly summarize findings, (2) call update_plan if needed, and \
                 (3) either apply_patch/write_file now or state exactly what missing information \
                 still blocks an edit.",
                self.model_turns_since_edit
            ));
        }

        if self.last_nudge_key != Some(key) {
            self.last_nudge_key = Some(key);
            if serial {
                self.serial_batch_nudges += 1;
            }
            if no_edit {
                self.no_edit_nudges += 1;
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
        // Retry injection does not double-count.
        let _ = t.nudge_for_injection();
        assert_eq!(t.serial_batch_nudges, 1);
    }

    #[test]
    fn batch_clears_serial_streak() {
        let mut t = ExplorationTracker::new();
        t.observe_turn(&[read()]);
        t.observe_turn(&[read()]);
        t.observe_turn(&[read(), search()]);
        assert_eq!(t.consecutive_single_readonly_turns, 0);
        assert!(t.nudge_for_injection().is_none());
    }

    #[test]
    fn no_edit_budget_and_reset_on_edit() {
        let mut t = ExplorationTracker::new();
        for _ in 0..NO_EDIT_BUDGET_TURNS {
            t.observe_turn(&[read()]);
        }
        let text = t.nudge_for_injection().unwrap();
        assert!(text.contains("No file edit"));
        t.note_edit();
        assert_eq!(t.model_turns_since_edit, 0);
        assert!(t.nudge_for_injection().is_none());
        let _ = patch(); // ensure Action used
    }

    #[test]
    fn avg_readonly_per_exploration() {
        let mut t = ExplorationTracker::new();
        t.observe_turn(&[read(), search()]);
        t.observe_turn(&[read()]);
        assert!((t.avg_readonly_tools_per_exploration_turn().unwrap() - 1.5).abs() < 1e-9);
    }
}
