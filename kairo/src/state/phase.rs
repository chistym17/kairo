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

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Phase::Explore => "explore",
            Phase::Act => "act",
            Phase::Verify => "verify",
            Phase::Finish => "finish",
        }
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
}
