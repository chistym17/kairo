//! Runtime phase control for the agent loop (slice 1–3).
//!
//! The model still chooses actions, but the runtime owns which phase the run is
//! in. Later slices filter the tool schema from [`Phase`] and transition
//! Explore → Act from progress / evidence-checkpoint signals.
//!
//! `Verify` and `Finish` exist so the enum is complete; their full behavior
//! (verify selection, auto-finish) lands in a later slice.

use std::collections::HashSet;
use std::path::PathBuf;

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
    /// Consecutive no-progress observations (slice 3 transitions at >= 2).
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

    pub fn has_seen_file(&self, path: &std::path::Path) -> bool {
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
    pub fn allow_act_read(&mut self, path: &std::path::Path) -> Result<(), String> {
        if self.has_seen_file(path) {
            return Ok(());
        }
        // Also accept if any seen path matches by file name equality after
        // normalizing to the same PathBuf form the caller passed.
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

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
}
