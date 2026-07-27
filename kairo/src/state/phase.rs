//! Runtime phase control for the agent loop (slice 1–3).
//!
//! The model still chooses actions, but the runtime owns which phase the run is
//! in. Tool schemas are filtered from [`Phase`] (slice 2). Explore → Act is
//! driven by progress / evidence-checkpoint signals (slice 3).
//!
//! `Verify` and `Finish` exist so the enum is complete; their full behavior
//! (verify selection, auto-finish) lands in a later slice.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::state::step::Action;
use crate::tools::ToolResult;

/// Consecutive no-progress observations that force Explore → Act.
pub const NO_PROGRESS_ACT_THRESHOLD: usize = 2;

/// Message injected when Explore → Act (capability change is the real control).
pub const ACT_TRANSITION_MESSAGE: &str = "\
Exploration is complete. Use the evidence already collected.
You must now update the plan, make a focused edit, or finish with a blocker.";

/// High-level loop phase. Tool availability is derived from this (slice 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Phase {
    #[default]
    Explore,
    Act,
    /// Stub for the next slice — not driven yet.
    Verify,
    /// Stub for the next slice — not driven yet.
    Finish,
}

/// Full explore toolkit (Level 0 — everything currently permitted).
pub const EXPLORE_TOOLS: &[&str] = &[
    "read_file",
    "list_files",
    "search_code",
    "locate",
    "find_files",
    "detect_project_snapshot",
    "summarize_repo",
    "git_status",
    "git_diff",
    "write_file",
    "apply_patch",
    "run_command",
    "run_shell",
    "update_plan",
    "finish",
];

/// Act phase: plan / patch / finish (+ limited reads). No search or shell.
pub const ACT_TOOLS: &[&str] = &["read_file", "update_plan", "apply_patch", "finish"];

/// Verify stub (next slice) — inspect + re-edit + finish.
pub const VERIFY_TOOLS: &[&str] = &[
    "git_status",
    "git_diff",
    "run_command",
    "run_shell",
    "apply_patch",
    "finish",
];

pub const FINISH_TOOLS: &[&str] = &["finish"];

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Phase::Explore => "explore",
            Phase::Act => "act",
            Phase::Verify => "verify",
            Phase::Finish => "finish",
        }
    }

    /// Tool names exposed to the model for this phase.
    pub fn allowed_tools(self) -> &'static [&'static str] {
        match self {
            Phase::Explore => EXPLORE_TOOLS,
            Phase::Act => ACT_TOOLS,
            Phase::Verify => VERIFY_TOOLS,
            Phase::Finish => FINISH_TOOLS,
        }
    }

    pub fn allows_tool(self, name: &str) -> bool {
        self.allowed_tools().contains(&name)
    }
}

/// Runtime-owned run control state (always on, independent of semantic_policy).
#[derive(Debug, Clone, Default)]
pub struct RunState {
    pub phase: Phase,
    /// Absolute or workspace-relative paths successfully read this run.
    pub seen_files: HashSet<PathBuf>,
    /// Stable search signatures already observed (tool + query + path scope).
    pub seen_searches: HashSet<String>,
    /// Consecutive no-progress observations (transitions at >= [`NO_PROGRESS_ACT_THRESHOLD`]).
    pub no_progress: usize,
    /// Act-mode: one justified read of an unseen file still allowed.
    pub justified_unseen_reads_left: u32,
}

impl RunState {
    pub fn new() -> Self {
        Self {
            phase: Phase::Explore,
            seen_files: HashSet::new(),
            seen_searches: HashSet::new(),
            no_progress: 0,
            justified_unseen_reads_left: 1,
        }
    }

    pub fn is_explore(&self) -> bool {
        matches!(self.phase, Phase::Explore)
    }

    pub fn is_act(&self) -> bool {
        matches!(self.phase, Phase::Act)
    }

    /// Record that a file was successfully read into context.
    pub fn note_file_seen(&mut self, path: PathBuf) {
        self.seen_files.insert(path);
    }

    pub fn has_seen_file(&self, path: &Path) -> bool {
        self.seen_files.contains(path)
    }

    /// Record a search signature. Returns `true` if this signature is new.
    pub fn note_search(&mut self, signature: impl Into<String>) -> bool {
        self.seen_searches.insert(signature.into())
    }

    /// Whether Act-mode may read this path (seen, or one justified unseen left).
    ///
    /// On allowing an unseen path, consumes the justified-unseen slot and records
    /// the path as seen so later Act reads of it are allowed.
    pub fn allow_act_read(&mut self, path: &Path) -> Result<(), String> {
        if self.has_seen_file(path) {
            return Ok(());
        }
        if self.justified_unseen_reads_left > 0 {
            self.justified_unseen_reads_left -= 1;
            self.note_file_seen(path.to_path_buf());
            return Ok(());
        }
        Err(
            "Act phase: read_file is limited to files already in context \
             (plus one justified new file). Update the plan, apply a patch, \
             or finish with a blocker."
                .to_string(),
        )
    }

    /// Enter Act if still exploring. Returns `true` when the phase changed.
    pub fn enter_act(&mut self) -> bool {
        if matches!(self.phase, Phase::Explore) {
            self.phase = Phase::Act;
            self.justified_unseen_reads_left = self.justified_unseen_reads_left.max(1);
            true
        } else {
            false
        }
    }

    /// Observe one tool outcome. May transition Explore → Act.
    ///
    /// Returns `Some(ACT_TRANSITION_MESSAGE)` when the phase just changed.
    pub fn observe_tool_outcome(
        &mut self,
        action: &Action,
        result: &ToolResult,
        duplicate_read: bool,
        evidence_checkpoint_active: bool,
    ) -> Option<&'static str> {
        if evidence_checkpoint_active && self.enter_act() {
            return Some(ACT_TRANSITION_MESSAGE);
        }

        if !matches!(self.phase, Phase::Explore) {
            // Still track seen files while in Act for read gating.
            self.record_side_effects(action, result, duplicate_read);
            return None;
        }

        let progressed = self.classify_progress(action, result, duplicate_read);
        if progressed {
            self.no_progress = 0;
        } else {
            self.no_progress = self.no_progress.saturating_add(1);
        }

        if self.no_progress >= NO_PROGRESS_ACT_THRESHOLD && self.enter_act() {
            Some(ACT_TRANSITION_MESSAGE)
        } else {
            None
        }
    }

    fn record_side_effects(
        &mut self,
        action: &Action,
        result: &ToolResult,
        duplicate_read: bool,
    ) {
        if let Action::ReadFile { path, .. } = action {
            if result.success && !duplicate_read {
                self.note_file_seen(path.clone());
            }
        }
        match action {
            Action::SearchCode { query, path } | Action::Locate { query, path } => {
                let sig = search_signature(action, query, path);
                let _ = self.note_search(sig);
            }
            Action::FindFiles { pattern, path } => {
                let sig = format!("find_files:{pattern}:@{}", path.display());
                let _ = self.note_search(sig);
            }
            _ => {}
        }
    }

    /// Returns `true` when this outcome counts as progress.
    fn classify_progress(
        &mut self,
        action: &Action,
        result: &ToolResult,
        duplicate_read: bool,
    ) -> bool {
        if is_denied_result(result) || duplicate_read {
            self.record_side_effects(action, result, duplicate_read);
            return false;
        }

        match action {
            Action::ReadFile { path, .. } => {
                if !result.success {
                    return false;
                }
                let path = path.clone();
                let is_new = !self.has_seen_file(&path);
                self.note_file_seen(path);
                is_new
            }
            Action::SearchCode { query, path } | Action::Locate { query, path } => {
                if !result.success {
                    return false;
                }
                let sig = search_signature(action, query, path);
                self.note_search(sig)
            }
            Action::FindFiles { pattern, path } => {
                if !result.success {
                    return false;
                }
                let sig = format!("find_files:{pattern}:@{}", path.display());
                self.note_search(sig)
            }
            Action::UpdatePlan { .. } if result.success => true,
            Action::ApplyPatch { .. } | Action::WriteFile { .. } if result.success => true,
            Action::RunCommand { cmd, .. } | Action::RunShell { command: cmd, .. } => {
                // Shell that only dumps an already-seen file = no progress.
                if let Some(intent) = crate::tools::shell_read::detect_shell_file_read(cmd) {
                    if self.has_seen_file(&intent.path) {
                        return false;
                    }
                    // First dump of a new path counts as seeing that file.
                    if result.success {
                        self.note_file_seen(intent.path);
                        return true;
                    }
                    return false;
                }
                result.success
            }
            Action::Finish { .. } => true,
            _ => result.success,
        }
    }
}

fn search_signature(action: &Action, query: &str, path: &Path) -> String {
    let kind = match action {
        Action::SearchCode { .. } => "search_code",
        Action::Locate { .. } => "locate",
        _ => "search",
    };
    format!("{kind}:{query}:@{}", path.display())
}

fn is_denied_result(result: &ToolResult) -> bool {
    if result.success {
        return false;
    }
    let Some(err) = result.error.as_deref() else {
        return true; // failed without detail — treat as no progress
    };
    let l = err.to_ascii_lowercase();
    l.contains("duplicate read denied")
        || l.contains("exact content already available")
        || l.contains("exploration checkpoint active")
        || l.contains("is not available in")
        || l.contains("phase_tool_denied")
        || l.contains("act phase: read_file")
        || l.contains("blocked:")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn ok_result() -> ToolResult {
        ToolResult {
            success: true,
            output: "ok".into(),
            diff: None,
            error: None,
            stop_reason: None,
        }
    }

    fn dup_read_result() -> ToolResult {
        ToolResult {
            success: false,
            output: String::new(),
            diff: None,
            error: Some("Duplicate read denied: Exact content already available".into()),
            stop_reason: None,
        }
    }

    fn read(path: &str) -> Action {
        Action::ReadFile {
            path: PathBuf::from(path),
            offset: 1,
            limit: 50,
        }
    }

    #[test]
    fn new_run_starts_in_explore() {
        let state = RunState::new();
        assert_eq!(state.phase, Phase::Explore);
        assert_eq!(state.no_progress, 0);
        assert!(state.seen_files.is_empty());
        assert!(state.seen_searches.is_empty());
        assert_eq!(state.justified_unseen_reads_left, 1);
    }

    #[test]
    fn note_file_seen_is_idempotent() {
        let mut state = RunState::new();
        let path = PathBuf::from("src/main.rs");
        state.note_file_seen(path.clone());
        state.note_file_seen(path.clone());
        assert_eq!(state.seen_files.len(), 1);
        assert!(state.has_seen_file(&path));
    }

    #[test]
    fn note_search_reports_novelty() {
        let mut state = RunState::new();
        assert!(state.note_search("search_code:foo:@."));
        assert!(!state.note_search("search_code:foo:@."));
        assert!(state.note_search("search_code:bar:@."));
        assert_eq!(state.seen_searches.len(), 2);
    }

    #[test]
    fn act_read_allows_seen_and_one_justified_unseen() {
        let mut state = RunState::new();
        state.phase = Phase::Act;
        let known = PathBuf::from("a.rs");
        state.note_file_seen(known.clone());
        assert!(state.allow_act_read(&known).is_ok());

        let first_new = PathBuf::from("b.rs");
        assert!(state.allow_act_read(&first_new).is_ok());
        assert_eq!(state.justified_unseen_reads_left, 0);
        assert!(state.has_seen_file(&first_new));

        let second_new = PathBuf::from("c.rs");
        assert!(state.allow_act_read(&second_new).is_err());
    }

    #[test]
    fn act_tools_exclude_search_and_shell() {
        assert!(!Phase::Act.allows_tool("search_code"));
        assert!(!Phase::Act.allows_tool("run_shell"));
        assert!(!Phase::Act.allows_tool("run_command"));
        assert!(Phase::Act.allows_tool("read_file"));
        assert!(Phase::Act.allows_tool("apply_patch"));
        assert!(Phase::Act.allows_tool("finish"));
    }

    #[test]
    fn duplicate_reads_twice_phase_becomes_act() {
        let mut state = RunState::new();
        let action = read("a.rs");
        // Seed as already seen so duplicates are clearly no-progress.
        state.note_file_seen(PathBuf::from("a.rs"));

        let msg1 = state.observe_tool_outcome(&action, &dup_read_result(), true, false);
        assert!(msg1.is_none());
        assert_eq!(state.phase, Phase::Explore);
        assert_eq!(state.no_progress, 1);

        let msg2 = state.observe_tool_outcome(&action, &dup_read_result(), true, false);
        assert_eq!(msg2, Some(ACT_TRANSITION_MESSAGE));
        assert_eq!(state.phase, Phase::Act);
        assert!(state.no_progress >= NO_PROGRESS_ACT_THRESHOLD);
    }

    #[test]
    fn new_files_are_read_stays_in_explore() {
        let mut state = RunState::new();
        for name in ["a.rs", "b.rs", "c.rs"] {
            let msg = state.observe_tool_outcome(&read(name), &ok_result(), false, false);
            assert!(msg.is_none(), "unexpected transition on {name}");
            assert_eq!(state.phase, Phase::Explore);
            assert_eq!(state.no_progress, 0);
        }
        assert_eq!(state.seen_files.len(), 3);
    }

    #[test]
    fn evidence_checkpoint_fires_phase_becomes_act() {
        let mut state = RunState::new();
        // Even with progress (new file), an active checkpoint forces Act.
        let msg = state.observe_tool_outcome(&read("fresh.rs"), &ok_result(), false, true);
        assert_eq!(msg, Some(ACT_TRANSITION_MESSAGE));
        assert_eq!(state.phase, Phase::Act);
    }
}
