//! Slice 3 (final-design-plan.md §7.1, §8): a single tagged stall counter,
//! running in **shadow mode** alongside the four counters it is a candidate
//! to eventually replace — `RunState::no_progress`,
//! `RunState::consecutive_denials`, `RunState::consecutive_invalid_actions`,
//! and `ProgressState::consecutive_no_progress`.
//!
//! Nothing reads `StallState` to gate behavior yet. It exists purely so a
//! transcript can log, per turn, whether the new unified counter would have
//! reached its threshold at the same point the old system actually stopped
//! (§8, Slice 3's definition of done). The old counters are removed only
//! after that comparison shows parity — Slice 4's job, not this one's.
//!
//! Coverage: both wiring points are live — committed tool outcomes
//! (`loop_engine::commit_tool_outcome`, covering all six `StallKind`s,
//! including a dispatched-but-empty search/read as `NonResolvingResponse`)
//! *and* text-only model turns with no tool call at all
//! (`loop_engine::run`'s `LlmError::NoToolCall` arm — a turn that never
//! reaches `commit_tool_outcome` at all, and so would otherwise never be
//! observed here; it is always classified `NonResolvingResponse`).
//! Fingerprinting a text-only turn has no `Action` to hash, so it uses
//! `ActionFingerprint::text_only` instead of `ActionFingerprint::of`.
//!
//! Two fingerprints, not one: `ActionFingerprint::of` ("did the model issue
//! the same call?") and `ActionFingerprint::coarse_of` ("is the model
//! trying the same kind of thing against the same target?"). A real
//! transcript showed a model dodging the exact-duplicate-read cache by
//! shifting its requested line range on every retry of one file — the
//! exact fingerprint correctly never called that a *repeat* (it wasn't),
//! but it was still the same stuck intent, which only the coarse
//! fingerprint's `read_file`-path-only identity catches. Both histories
//! are diagnostic-only; neither drives `consecutive_stalls`, which is
//! classification-driven (`StallKind`), not fingerprint-driven.
//!
//! `RecoveryState` (Slice 4 Part 1) is deliberately *not* folded into this
//! counter: a successful recovery reread resets `consecutive_stalls` while
//! leaving `Recovery.consecutive_failures` unchanged.

use std::collections::VecDeque;
use std::hash::{Hash, Hasher};

use crate::state::step::Action;

/// Ring-buffer capacity for `stall_history` (final-design-plan.md §7.1: "3-4").
const STALL_HISTORY_CAPACITY: usize = 4;

/// Why a turn was classified as a stall (no useful change in what the model
/// can now do). Mirrors final-design-plan.md §7.1 exactly — six variants,
/// composable with `ActionFingerprint` history rather than tool-specific.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StallKind {
    /// A read/search/list that returned content already in context.
    DuplicateEvidence,
    /// Tool/argument rejected by the current phase, recovery, or
    /// authorization scope (not by argument shape).
    ScopeViolation,
    /// Rejected for malformed arguments (shape validation, empty fields).
    InvalidArguments,
    /// `apply_patch` / `write_file` dispatched and failed.
    FailedMutation,
    /// A verification command that repeated a result already seen.
    RepeatedVerification,
    /// Dispatched successfully but produced no new evidence or decision
    /// (catch-all — e.g. a fresh search with zero matches).
    NonResolvingResponse,
}

/// Cheap per-action identity used to detect repeats/ping-pong in
/// `stall_history`. Two actions fingerprint equal when they would look like
/// the same retried request to the model: same tool + same primary target.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ActionFingerprint(String);

impl ActionFingerprint {
    pub fn of(action: &Action) -> Self {
        let s = match action {
            Action::ReadFile { path, offset, limit } => {
                format!("read_file:{}:{offset}:{limit}", path.display())
            }
            Action::ListFiles { path } => format!("list_files:{}", path.display()),
            Action::SearchCode { query, path } => format!("search_code:{query}:{}", path.display()),
            Action::Locate { query, path } => format!("locate:{query}:{}", path.display()),
            Action::FindFiles { pattern, path } => format!("find_files:{pattern}:{}", path.display()),
            Action::DetectProjectSnapshot { path } => {
                format!("detect_project_snapshot:{}", path.display())
            }
            Action::SummarizeRepo { depth, path, .. } => {
                format!("summarize_repo:{depth}:{}", path.display())
            }
            Action::GitStatus => "git_status".to_string(),
            Action::GitDiff { path } => format!(
                "git_diff:{}",
                path.as_deref().map(|p| p.display().to_string()).unwrap_or_default()
            ),
            Action::WriteFile { path, .. } => format!("write_file:{}", path.display()),
            Action::ApplyPatch { path, .. } => format!("apply_patch:{}", path.display()),
            Action::RunCommand { cmd, cwd, .. } => format!("run_command:{cmd}:{}", cwd.display()),
            Action::RunShell { command, cwd, .. } => format!("run_shell:{command}:{}", cwd.display()),
            Action::UpdatePlan { .. } => "update_plan".to_string(),
            Action::Finish { .. } => "finish".to_string(),
        };
        ActionFingerprint(s)
    }

    /// Fingerprint for a text-only model turn (no tool call at all — the
    /// `LlmError::NoToolCall` path in `loop_engine::run`, which never
    /// reaches `commit_tool_outcome` and so has no `Action` to fingerprint
    /// off of). Hashes the returned text rather than storing it verbatim,
    /// since two near-identical essays should still both register as
    /// `NonResolvingResponse` stalls even if `is_repeat()` doesn't fire for
    /// them (essay wording rarely repeats byte-for-byte; the point of this
    /// fingerprint is dedup/repeat detection, not the count itself).
    pub fn text_only(text: &str) -> Self {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut h);
        ActionFingerprint(format!("no_tool_call:{:x}", h.finish()))
    }

    /// Coarser, "same intent against the same target" identity — diagnostic
    /// only, logged alongside `of`'s exact identity, never a replacement for
    /// it. Currently scoped to `read_file` only (drops `offset`/`limit`,
    /// keeping just the path): a real transcript showed a model dodging the
    /// exact-duplicate-read cache by shifting its requested line range on
    /// every retry of the same file, which the exact fingerprint correctly
    /// never flagged as a *repeat* (it wasn't — the ranges genuinely
    /// differed) but which is still the same stuck intent. Every other
    /// action currently coarsens to the same value as `of` — deliberately
    /// not generalized to search/verify/patch yet (see module docs); that
    /// is follow-up work once shadow-mode data justifies a specific
    /// coarsening rule for each of those, rather than guessed here from one
    /// transcript.
    pub fn coarse_of(action: &Action) -> Self {
        match action {
            Action::ReadFile { path, .. } => ActionFingerprint(format!("read_file:{}", path.display())),
            other => Self::of(other),
        }
    }
}

type StallHistory = VecDeque<(ActionFingerprint, StallKind)>;

#[derive(Debug, Default, Clone)]
pub struct StallState {
    pub consecutive_stalls: u32,
    /// "Did the model issue the same call?" — exact tool + arguments.
    pub stall_history: StallHistory,
    /// "Is the model trying the same kind of thing against the same
    /// target?" — coarser identity (`ActionFingerprint::coarse_of`),
    /// diagnostic-only, logged alongside the exact history rather than
    /// replacing it (see that constructor's doc comment for why).
    pub coarse_stall_history: StallHistory,
}

impl StallState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one turn's classification against already-computed exact and
    /// coarse fingerprints. `stall` is `None` for progress (mutation,
    /// meaningful plan update, genuinely new evidence, a useful
    /// verification result, or a `finish`/decision) and `Some(kind)` for
    /// anything the legacy counters would have treated as unproductive.
    ///
    /// `None` must never be passed merely because a tool call *succeeded*,
    /// or because a search/read was *syntactically* different from the
    /// last one — see `loop_engine::classify_stall`'s doc comment for the
    /// exact boundary this relies on (`ProgressKind`, shared with the
    /// legacy `consecutive_no_progress` counter this is compared against).
    ///
    /// Shadow mode only: this never returns a stop signal, and nothing
    /// currently reads `consecutive_stalls` (or either history) to change
    /// runtime behavior.
    pub fn observe_fingerprints(
        &mut self,
        exact: ActionFingerprint,
        coarse: ActionFingerprint,
        stall: Option<StallKind>,
    ) {
        match stall {
            Some(kind) => {
                self.consecutive_stalls = self.consecutive_stalls.saturating_add(1);
                push_bounded(&mut self.stall_history, (exact, kind));
                push_bounded(&mut self.coarse_stall_history, (coarse, kind));
            }
            None => {
                self.consecutive_stalls = 0;
                self.stall_history.clear();
                self.coarse_stall_history.clear();
            }
        }
    }

    /// Single-fingerprint convenience for turns with no meaningful
    /// exact/coarse distinction yet (currently: text-only turns — see
    /// `ActionFingerprint::text_only`). Uses the same fingerprint for both
    /// histories.
    pub fn observe_fingerprint(&mut self, fingerprint: ActionFingerprint, stall: Option<StallKind>) {
        self.observe_fingerprints(fingerprint.clone(), fingerprint, stall);
    }

    /// Convenience wrapper for the (common) case of a committed tool
    /// outcome, where an `Action` is available to fingerprint directly.
    pub fn observe(&mut self, action: &Action, stall: Option<StallKind>) {
        self.observe_fingerprints(
            ActionFingerprint::of(action),
            ActionFingerprint::coarse_of(action),
            stall,
        );
    }

    /// A, A, A — stuck repeating the identical action (exact identity).
    pub fn is_repeat(&self) -> bool {
        history_is_repeat(&self.stall_history)
    }

    /// A, B, A, B — oscillating between two distinct actions (exact identity).
    pub fn is_ping_pong(&self) -> bool {
        history_is_ping_pong(&self.stall_history)
    }

    /// Each recorded attempt is distinct (exact identity) — a genuinely
    /// hard case, closer to a real blocker than a mechanical loop.
    pub fn is_new_failure(&self) -> bool {
        history_is_new_failure(&self.stall_history)
    }

    /// Same as `is_repeat`, but on the coarse ("same target, same intent")
    /// identity — e.g. catches repeated reads of one file at shifting line
    /// ranges, which `is_repeat` (exact) will not.
    pub fn coarse_is_repeat(&self) -> bool {
        history_is_repeat(&self.coarse_stall_history)
    }

    pub fn coarse_is_ping_pong(&self) -> bool {
        history_is_ping_pong(&self.coarse_stall_history)
    }

    pub fn coarse_is_new_failure(&self) -> bool {
        history_is_new_failure(&self.coarse_stall_history)
    }
}

fn push_bounded(history: &mut StallHistory, entry: (ActionFingerprint, StallKind)) {
    if history.len() >= STALL_HISTORY_CAPACITY {
        history.pop_front();
    }
    history.push_back(entry);
}

fn history_is_repeat(history: &StallHistory) -> bool {
    history.len() >= 2 && history.iter().all(|(fp, _)| *fp == history[0].0)
}

fn history_is_ping_pong(history: &StallHistory) -> bool {
    if history.len() < 4 {
        return false;
    }
    let n = history.len();
    let last = &history[n - 1].0;
    let prev = &history[n - 2].0;
    let prev2 = &history[n - 3].0;
    let prev3 = &history[n - 4].0;
    last != prev && last == prev2 && prev == prev3
}

fn history_is_new_failure(history: &StallHistory) -> bool {
    history.len() >= 2 && !history_is_repeat(history) && !history_is_ping_pong(history)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn read(path: &str) -> Action {
        Action::ReadFile {
            path: PathBuf::from(path),
            offset: 1,
            limit: 50,
        }
    }

    fn read_range(path: &str, offset: u32, limit: u32) -> Action {
        Action::ReadFile {
            path: PathBuf::from(path),
            offset,
            limit,
        }
    }

    fn search(query: &str) -> Action {
        Action::SearchCode {
            query: query.into(),
            path: PathBuf::from("."),
        }
    }

    #[test]
    fn new_state_has_no_stalls() {
        let s = StallState::new();
        assert_eq!(s.consecutive_stalls, 0);
        assert!(s.stall_history.is_empty());
    }

    #[test]
    fn progress_resets_streak_and_history() {
        let mut s = StallState::new();
        s.observe(&read("a.rs"), Some(StallKind::DuplicateEvidence));
        s.observe(&read("a.rs"), Some(StallKind::DuplicateEvidence));
        assert_eq!(s.consecutive_stalls, 2);

        s.observe(&read("b.rs"), None);
        assert_eq!(s.consecutive_stalls, 0);
        assert!(s.stall_history.is_empty());
        assert!(s.coarse_stall_history.is_empty());
    }

    #[test]
    fn stall_increments_and_records_fingerprint() {
        let mut s = StallState::new();
        s.observe(&read("a.rs"), Some(StallKind::DuplicateEvidence));
        assert_eq!(s.consecutive_stalls, 1);
        assert_eq!(s.stall_history.len(), 1);
        assert_eq!(s.stall_history[0].1, StallKind::DuplicateEvidence);
    }

    #[test]
    fn history_ring_buffer_is_bounded() {
        let mut s = StallState::new();
        for i in 0..(STALL_HISTORY_CAPACITY + 3) {
            let path = format!("f{i}.rs");
            s.observe(&read(&path), Some(StallKind::NonResolvingResponse));
        }
        assert_eq!(s.consecutive_stalls, (STALL_HISTORY_CAPACITY + 3) as u32);
        assert_eq!(s.stall_history.len(), STALL_HISTORY_CAPACITY);
        assert_eq!(s.coarse_stall_history.len(), STALL_HISTORY_CAPACITY);
    }

    #[test]
    fn is_repeat_detects_identical_fingerprints() {
        let mut s = StallState::new();
        for _ in 0..3 {
            s.observe(&read("a.rs"), Some(StallKind::DuplicateEvidence));
        }
        assert!(s.is_repeat());
        assert!(!s.is_ping_pong());
        assert!(!s.is_new_failure());
    }

    #[test]
    fn is_ping_pong_detects_alternating_fingerprints() {
        let mut s = StallState::new();
        s.observe(&read("a.rs"), Some(StallKind::DuplicateEvidence));
        s.observe(&search("x"), Some(StallKind::NonResolvingResponse));
        s.observe(&read("a.rs"), Some(StallKind::DuplicateEvidence));
        s.observe(&search("x"), Some(StallKind::NonResolvingResponse));
        assert!(s.is_ping_pong());
        assert!(!s.is_repeat());
    }

    #[test]
    fn is_new_failure_detects_all_distinct_attempts() {
        let mut s = StallState::new();
        s.observe(&read("a.rs"), Some(StallKind::DuplicateEvidence));
        s.observe(&read("b.rs"), Some(StallKind::InvalidArguments));
        s.observe(&read("c.rs"), Some(StallKind::ScopeViolation));
        assert!(s.is_new_failure());
        assert!(!s.is_repeat());
        assert!(!s.is_ping_pong());
    }

    #[test]
    fn single_stall_is_neither_repeat_nor_ping_pong_nor_new_failure() {
        let mut s = StallState::new();
        s.observe(&read("a.rs"), Some(StallKind::DuplicateEvidence));
        assert!(!s.is_repeat());
        assert!(!s.is_ping_pong());
        assert!(!s.is_new_failure());
    }

    #[test]
    fn text_only_fingerprint_is_stable_and_content_sensitive() {
        assert_eq!(
            ActionFingerprint::text_only("same essay"),
            ActionFingerprint::text_only("same essay")
        );
        assert_ne!(
            ActionFingerprint::text_only("essay one"),
            ActionFingerprint::text_only("essay two")
        );
    }

    #[test]
    fn coarse_read_fingerprint_ignores_offset_and_limit() {
        let a = Action::ReadFile {
            path: PathBuf::from("large.rs"),
            offset: 1,
            limit: 200,
        };
        let b = Action::ReadFile {
            path: PathBuf::from("large.rs"),
            offset: 800,
            limit: 200,
        };
        assert_ne!(ActionFingerprint::of(&a), ActionFingerprint::of(&b));
        assert_eq!(ActionFingerprint::coarse_of(&a), ActionFingerprint::coarse_of(&b));
    }

    #[test]
    fn coarse_fingerprint_matches_exact_for_non_read_actions() {
        let a = search("x");
        assert_eq!(ActionFingerprint::of(&a), ActionFingerprint::coarse_of(&a));
    }

    #[test]
    fn shifting_read_ranges_are_exact_new_failure_but_coarse_repeat() {
        let mut s = StallState::new();
        s.observe(&read_range("large.rs", 1, 200), Some(StallKind::DuplicateEvidence));
        s.observe(&read_range("large.rs", 800, 200), Some(StallKind::DuplicateEvidence));
        s.observe(&read_range("large.rs", 400, 200), Some(StallKind::DuplicateEvidence));

        // Exact identity: three genuinely different requests.
        assert!(s.is_new_failure());
        assert!(!s.is_repeat());

        // Coarse identity: same file every time — a repeat in intent.
        assert!(s.coarse_is_repeat());
        assert!(!s.coarse_is_new_failure());
    }

    #[test]
    fn progressive_reads_of_one_large_file_are_progress_not_stalls() {
        // Genuinely new evidence each time (different ranges, both
        // classified `None` upstream by `classify_stall`) must reset both
        // histories — coarse identity must never turn a legitimately
        // productive progressive read into a manufactured "repeat".
        let mut s = StallState::new();
        s.observe(&read_range("large.rs", 1, 200), None);
        s.observe(&read_range("large.rs", 200, 200), None);
        assert_eq!(s.consecutive_stalls, 0);
        assert!(s.coarse_stall_history.is_empty());
    }

    #[test]
    fn text_only_turns_count_as_non_resolving_stalls() {
        let mut s = StallState::new();
        s.observe_fingerprint(
            ActionFingerprint::text_only("here's what I found..."),
            Some(StallKind::NonResolvingResponse),
        );
        s.observe_fingerprint(
            ActionFingerprint::text_only("here's what I found..."),
            Some(StallKind::NonResolvingResponse),
        );
        assert_eq!(s.consecutive_stalls, 2);
        assert!(s.is_repeat());
    }
}
