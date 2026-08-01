//! Layer 1 `Recovery` — bounded subsystem state for failed mutations
//! (final-design-plan.md §4, §8 Slice 4 Part 1).
//!
//! Absorbs what used to be split across `PatchRecoveryState` (grant +
//! failure/write budgets) and `RunState::{patch_fail_recovery, patch_fail_path}`
//! (tool-mode lock). Those were always updated together; keeping them in two
//! places made "is recovery active?" and "how many failures on this target?"
//! answerable from different structs — the Issue 3 corpus finding that Slice 3
//! corrected architecturally.
//!
//! **Not** a general stuckness counter. `StallState` / `Progress` answers
//! "is the agent generally making progress?"; this answers "has this specific
//! mutation target repeatedly failed despite a bounded courtesy retry?" A
//! successful recovery reread resets the former and must leave this one's
//! `consecutive_failures` untouched.

use std::path::{Path, PathBuf};

use crate::state::stop_reason::StopReason;
use crate::tools::ToolResult;

/// Stop after this many consecutive `apply_patch` failures on the same file.
pub const MAX_CONSECUTIVE_PATCH_FAILURES: u32 = 3;

/// Stop after this many denied `write_file` attempts on existing source files.
pub const MAX_DENIED_SOURCE_WRITES: u32 = 2;

/// Extensions treated as source (existing files must use `apply_patch`).
const SOURCE_EXTENSIONS: &[&str] = &[
    "js", "mjs", "cjs", "ts", "tsx", "jsx", "py", "rs", "go", "java", "kt", "kts", "c",
    "cc", "cpp", "cxx", "h", "hpp", "hh", "rb", "php", "swift", "cs", "scala", "vue",
    "svelte",
];

/// Why recovery is currently active. Part 1 only engages `FailedPatch`;
/// later Slice 4 recipes may add variants without changing the field layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryReason {
    FailedPatch,
}

/// Read-only snapshot for shadow-mode telemetry (and any later Layer 2
/// derivation). Never used to gate behavior by itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoverySnapshot {
    pub active: bool,
    pub reason: Option<RecoveryReason>,
    pub target: Option<PathBuf>,
    pub consecutive_failures: u32,
    pub remaining_reads: u32,
    pub denied_source_writes: u32,
}

/// Outcome of [`RecoveryState::after_patch_failure`].
#[derive(Debug)]
pub struct PatchFailureOutcome {
    pub result: ToolResult,
    /// True when recovery mode newly engaged or the failed target changed —
    /// callers should queue phase guidance once for that transition.
    pub show_phase_guidance: bool,
}

#[derive(Debug, Default, Clone)]
pub struct RecoveryState {
    reason: Option<RecoveryReason>,
    target: Option<PathBuf>,
    remaining_reads: u32,
    consecutive_failures: u32,
    denied_source_writes: u32,
}

impl RecoveryState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_active(&self) -> bool {
        self.reason.is_some()
    }

    pub fn reason(&self) -> Option<RecoveryReason> {
        self.reason
    }

    pub fn target(&self) -> Option<&Path> {
        self.target.as_deref()
    }

    pub fn remaining_reads(&self) -> u32 {
        self.remaining_reads
    }

    pub fn consecutive_failures(&self) -> u32 {
        self.consecutive_failures
    }

    pub fn denied_source_writes(&self) -> u32 {
        self.denied_source_writes
    }

    pub fn snapshot(&self) -> RecoverySnapshot {
        RecoverySnapshot {
            active: self.is_active(),
            reason: self.reason,
            target: self.target.clone(),
            consecutive_failures: self.consecutive_failures,
            remaining_reads: self.remaining_reads,
            denied_source_writes: self.denied_source_writes,
        }
    }

    pub fn has_grant_for(&self, path: &Path) -> bool {
        self.remaining_reads > 0
            && self
                .target
                .as_ref()
                .is_some_and(|p| paths_equal(p, path))
    }

    /// Consume one recovery read if this path matches the grant.
    /// Returns true when the read should bypass duplicate-read denial.
    ///
    /// Does **not** clear `reason` / `target` / `consecutive_failures` — the
    /// courtesy reread is progress for the general stall counter, not a reset
    /// of this mutation-failure budget.
    pub fn try_consume_read(&mut self, path: &Path) -> bool {
        if !self.has_grant_for(path) {
            return false;
        }
        self.remaining_reads = self.remaining_reads.saturating_sub(1);
        true
    }

    /// Record a successful mutation — clears recovery mode and failure streak.
    pub fn note_successful_mutation(&mut self) {
        self.clear();
    }

    /// Clear recovery mode (e.g. entering Finish). Same field wipe as a
    /// successful mutation.
    pub fn clear(&mut self) {
        self.reason = None;
        self.target = None;
        self.remaining_reads = 0;
        self.consecutive_failures = 0;
        // denied_source_writes is a run-lifetime budget, not tied to one
        // recovery episode — leave it alone (matches prior PatchRecoveryState).
    }

    /// After a failed `apply_patch`: activate recovery, grant one reread, and
    /// maybe terminate when the same-target failure budget is exhausted.
    pub fn after_patch_failure(&mut self, path: &Path, mut result: ToolResult) -> PatchFailureOutcome {
        let path_buf = path.to_path_buf();
        let show_phase_guidance = !(self.is_active()
            && self
                .target
                .as_ref()
                .is_some_and(|p| paths_equal(p, path)));

        if self
            .target
            .as_ref()
            .is_some_and(|p| paths_equal(p, path))
            && self.consecutive_failures > 0
        {
            self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        } else {
            self.consecutive_failures = 1;
        }

        self.reason = Some(RecoveryReason::FailedPatch);
        self.target = Some(path_buf);
        self.remaining_reads = 1;

        let recovery = format!(
            "\n\nPatch failed because the expected context did not match (or made no change).\n\
             You may reread this file once to refresh the relevant section:\n\
             {}\n\
             After rereading, submit a smaller apply_patch against the current content.\n\
             Do not rewrite the entire file with write_file.",
            path.display()
        );
        match &mut result.error {
            Some(e) => e.push_str(&recovery),
            None => result.error = Some(recovery.trim_start().to_string()),
        }

        if self.consecutive_failures >= MAX_CONSECUTIVE_PATCH_FAILURES {
            let msg = format!(
                "Stopped: {MAX_CONSECUTIVE_PATCH_FAILURES} consecutive apply_patch failures on {} \
                 without a successful edit. Regenerate from a fresh read or finish with blockers.",
                path.display()
            );
            if let Some(e) = &mut result.error {
                e.push_str("\n\n");
                e.push_str(&msg);
            }
            result.stop_reason = Some(StopReason::NoProgress);
        }

        PatchFailureOutcome {
            result,
            show_phase_guidance,
        }
    }

    /// Deny `write_file` when the path already exists as source code.
    pub fn deny_write_existing_source(
        &mut self,
        workspace_root: &Path,
        path: &Path,
    ) -> Option<ToolResult> {
        if !is_source_path(path) {
            return None;
        }
        let full = workspace_root.join(path);
        if !full.is_file() {
            return None;
        }

        self.denied_source_writes = self.denied_source_writes.saturating_add(1);

        let mut message = format!(
            "write_file denied for existing source file {}.\n\
             Use apply_patch for surgical edits. Do not replace the whole file.\n\
             If a patch just failed, read_file that path once (recovery grant), then apply_patch again.",
            path.display()
        );
        if self.has_grant_for(path) {
            message.push_str(&format!(
                "\nA recovery reread is currently available for {}.",
                path.display()
            ));
        }

        let mut result = ToolResult::error(message);
        if self.denied_source_writes >= MAX_DENIED_SOURCE_WRITES {
            result.error = Some(format!(
                "{}\n\nStopped: {MAX_DENIED_SOURCE_WRITES} denied write_file attempts on existing \
                 source files. Use apply_patch or finish with blockers.",
                result.error.as_deref().unwrap_or("")
            ));
            result.stop_reason = Some(StopReason::NoProgress);
        }
        Some(result)
    }
}

pub fn is_source_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| SOURCE_EXTENSIONS.iter().any(|s| e.eq_ignore_ascii_case(s)))
        .unwrap_or(false)
}

fn paths_equal(a: &Path, b: &Path) -> bool {
    normalize(a) == normalize(b)
}

fn normalize(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .trim_start_matches("./")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::stall::{StallKind, StallState};
    use crate::state::step::Action;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn grant_allows_one_read_then_expires_without_clearing_mode() {
        let mut s = RecoveryState::new();
        let path = Path::new("src/store.js");
        let _ = s.after_patch_failure(path, ToolResult::error("hunk mismatch"));
        assert!(s.is_active());
        assert!(s.has_grant_for(path));
        assert!(s.try_consume_read(path));
        assert!(!s.has_grant_for(path));
        assert_eq!(s.remaining_reads(), 0);
        // Mode + failure streak survive the courtesy reread.
        assert!(s.is_active());
        assert_eq!(s.consecutive_failures(), 1);
        assert_eq!(s.target(), Some(path));
        assert!(!s.try_consume_read(path));
    }

    #[test]
    fn third_consecutive_failure_stops() {
        let mut s = RecoveryState::new();
        let path = Path::new("a.js");
        for i in 1..=3 {
            let out = s.after_patch_failure(path, ToolResult::error(format!("fail {i}")));
            if i < 3 {
                assert!(out.result.stop_reason.is_none());
            } else {
                assert_eq!(out.result.stop_reason, Some(StopReason::NoProgress));
            }
        }
    }

    #[test]
    fn first_failure_requests_phase_guidance_repeat_same_path_does_not() {
        let mut s = RecoveryState::new();
        let path = Path::new("a.js");
        let first = s.after_patch_failure(path, ToolResult::error("1"));
        assert!(first.show_phase_guidance);
        let second = s.after_patch_failure(path, ToolResult::error("2"));
        assert!(!second.show_phase_guidance);
        let other = s.after_patch_failure(Path::new("b.js"), ToolResult::error("other"));
        assert!(other.show_phase_guidance);
        assert_eq!(s.consecutive_failures(), 1);
    }

    #[test]
    fn success_clears_streak() {
        let mut s = RecoveryState::new();
        let path = Path::new("a.js");
        let _ = s.after_patch_failure(path, ToolResult::error("1"));
        let _ = s.after_patch_failure(path, ToolResult::error("2"));
        s.note_successful_mutation();
        assert!(!s.is_active());
        let out = s.after_patch_failure(path, ToolResult::error("again"));
        assert_eq!(s.consecutive_failures(), 1);
        assert!(out.result.stop_reason.is_none());
    }

    #[test]
    fn denies_write_on_existing_js() {
        let dir = tempdir().unwrap();
        let rel = Path::new("src/store.js");
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join(rel), "export const x = 1;\n").unwrap();
        let mut s = RecoveryState::new();
        let denied = s.deny_write_existing_source(dir.path(), rel);
        assert!(denied.is_some());
        assert!(!denied.unwrap().success);
    }

    #[test]
    fn allows_write_for_new_source_file() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        let mut s = RecoveryState::new();
        assert!(s
            .deny_write_existing_source(dir.path(), Path::new("src/new.js"))
            .is_none());
    }

    #[test]
    fn snapshot_reflects_active_failure_streak_and_grant() {
        let mut s = RecoveryState::new();
        let path = Path::new("a.js");
        let empty = s.snapshot();
        assert!(!empty.active);
        assert_eq!(empty.target, None);

        let _ = s.after_patch_failure(path, ToolResult::error("hunk mismatch"));
        let snap = s.snapshot();
        assert!(snap.active);
        assert_eq!(snap.reason, Some(RecoveryReason::FailedPatch));
        assert_eq!(snap.target, Some(PathBuf::from("a.js")));
        assert_eq!(snap.consecutive_failures, 1);
        assert_eq!(snap.remaining_reads, 1);

        s.note_successful_mutation();
        let cleared = s.snapshot();
        assert!(!cleared.active);
        assert_eq!(cleared.consecutive_failures, 0);
        assert_eq!(cleared.remaining_reads, 0);
    }

    #[test]
    fn allows_write_for_existing_json() {
        let dir = tempdir().unwrap();
        let rel = Path::new("package.json");
        fs::write(dir.path().join(rel), "{}\n").unwrap();
        let mut s = RecoveryState::new();
        assert!(s.deny_write_existing_source(dir.path(), rel).is_none());
    }

    /// Slice 4 Part 1 definition-of-done regression (Issue 3 corpus finding):
    /// a successful recovery reread resets general stalls but must leave
    /// `Recovery.consecutive_failures` unchanged; the third failed mutation
    /// still terminates via Recovery alone.
    #[test]
    fn recovery_reread_resets_stalls_not_failure_budget() {
        let mut recovery = RecoveryState::new();
        let mut stall = StallState::new();
        let path = PathBuf::from("frontend-v2/src/lib/task-status.ts");
        let patch = Action::ApplyPatch {
            path: path.clone(),
            diff: "x".into(),
        };
        let read = Action::ReadFile {
            path: path.clone(),
            offset: 1,
            limit: 30,
        };

        let _ = recovery.after_patch_failure(&path, ToolResult::error("fail 1"));
        stall.observe(&patch, Some(StallKind::FailedMutation));
        let _ = recovery.after_patch_failure(&path, ToolResult::error("fail 2"));
        stall.observe(&patch, Some(StallKind::FailedMutation));
        assert_eq!(recovery.consecutive_failures(), 2);
        assert_eq!(stall.consecutive_stalls, 2);

        assert!(recovery.try_consume_read(&path));
        // Genuine new evidence — Progress/Stall resets.
        stall.observe(&read, None);
        assert_eq!(stall.consecutive_stalls, 0);
        // Recovery budget untouched.
        assert_eq!(recovery.consecutive_failures(), 2);
        assert_eq!(recovery.remaining_reads(), 0);
        assert!(recovery.is_active());

        let third = recovery.after_patch_failure(&path, ToolResult::error("fail 3"));
        assert_eq!(recovery.consecutive_failures(), 3);
        assert_eq!(third.result.stop_reason, Some(StopReason::NoProgress));
    }
}
