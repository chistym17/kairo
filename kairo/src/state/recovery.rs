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

use std::collections::HashMap;
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
    /// Last general-recipe key that produced recovery guidance (not the
    /// FailedPatch budget — that is `consecutive_failures` + `target`).
    guidance_key: Option<RecoveryKey>,
    /// Attempts for `guidance_key` only. Reset when the key changes.
    guidance_attempts: u32,
    /// One-shot MissingEvidence read grant — does **not** engage FailedPatch
    /// recovery mode (`is_active()` stays false). Separate from
    /// `remaining_reads` / `target` so Act tooling is not locked to
    /// `PATCH_RECOVERY_TOOLS`.
    evidence_grant_path: Option<PathBuf>,
    evidence_grant_remaining: u32,
    evidence_grant_key: Option<RecoveryKey>,
    /// Act-phase duplicate-evidence escalation per path (Part 5B).
    /// Level 1 = cached/duplicate ranges blocked; level ≥2 = path disabled for
    /// `read_file`. Does **not** set [`RecoveryReason`] / `is_active()` — that
    /// remains FailedPatch-only so Act keeps apply_patch/update_plan/finish.
    duplicate_escalation: HashMap<String, u32>,
    /// Exact file lines from the last FailedPatch mismatch (Slice 5G).
    /// Next apply_patch on this target must cite ≥1 of these as context/removal.
    grounded_lines: Vec<String>,
    /// Formatted excerpt shown in recovery guidance / ungrounded rejects.
    grounded_display: Option<String>,
}

/// Identity for a bounded recovery-guidance sequence.
///
/// Same reason+tool+target → escalate attempt 1, 2, 3.
/// Different reason or target → start a new sequence at attempt 1.
/// Broader oscillation across keys is Part 4's job via stall fingerprints.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RecoveryKey {
    pub reason: String,
    pub tool: String,
    pub target: Option<PathBuf>,
}

impl RecoveryKey {
    pub fn new(
        reason: impl Into<String>,
        tool: impl Into<String>,
        target: Option<PathBuf>,
    ) -> Self {
        Self {
            reason: reason.into(),
            tool: tool.into(),
            target,
        }
    }
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
        self.guidance_key = None;
        self.guidance_attempts = 0;
        self.evidence_grant_path = None;
        self.evidence_grant_remaining = 0;
        self.evidence_grant_key = None;
        self.duplicate_escalation.clear();
        self.grounded_lines.clear();
        self.grounded_display = None;
        // denied_source_writes is a run-lifetime budget, not tied to one
        // recovery episode — leave it alone (matches prior PatchRecoveryState).
    }

    /// Clear only Act duplicate-evidence path restrictions (e.g. after a
    /// successful `update_plan`). Leaves FailedPatch budgets untouched.
    pub fn clear_duplicate_escalation(&mut self) {
        self.duplicate_escalation.clear();
    }

    /// Record one Act duplicate-evidence hit for `path`. Returns the new level
    /// (1 = restrict cached ranges, 2+ = disable path).
    pub fn note_act_duplicate(&mut self, path: &Path) -> u32 {
        let key = normalize(path);
        let entry = self.duplicate_escalation.entry(key).or_insert(0);
        *entry = (*entry).saturating_add(1);
        *entry
    }

    pub fn duplicate_escalation_level(&self, path: &Path) -> u32 {
        self.duplicate_escalation
            .get(&normalize(path))
            .copied()
            .unwrap_or(0)
    }

    /// Level ≥1: further *cached* reads of this path should be rejected in Act.
    pub fn is_act_duplicate_restricted(&self, path: &Path) -> bool {
        self.duplicate_escalation_level(path) >= 1
    }

    /// Level ≥2: any `read_file` of this path is disabled in Act.
    pub fn is_act_duplicate_path_disabled(&self, path: &Path) -> bool {
        self.duplicate_escalation_level(path) >= 2
    }

    /// Paths currently under duplicate-evidence restriction (for snapshots).
    pub fn duplicate_escalation_paths(&self) -> Vec<(PathBuf, u32)> {
        let mut out: Vec<_> = self
            .duplicate_escalation
            .iter()
            .map(|(p, lvl)| (PathBuf::from(p), *lvl))
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// Record one application of a general recovery recipe keyed by
    /// reason/tool/target. Returns the attempt number for this key (1-based).
    ///
    /// Does **not** affect `consecutive_failures` (FailedPatch budget).
    pub fn note_guidance_attempt(&mut self, key: RecoveryKey) -> u32 {
        if self.guidance_key.as_ref() == Some(&key) {
            self.guidance_attempts = self.guidance_attempts.saturating_add(1);
        } else {
            self.guidance_key = Some(key);
            self.guidance_attempts = 1;
        }
        self.guidance_attempts
    }

    pub fn guidance_attempts(&self) -> u32 {
        self.guidance_attempts
    }

    pub fn guidance_key(&self) -> Option<&RecoveryKey> {
        self.guidance_key.as_ref()
    }

    pub fn has_evidence_grant_for(&self, path: &Path) -> bool {
        self.evidence_grant_remaining > 0
            && self
                .evidence_grant_path
                .as_ref()
                .is_some_and(|p| paths_equal(p, path))
    }

    /// Grant one bounded MissingEvidence read for a normalized workspace-relative
    /// path. Does not engage FailedPatch mode. Same `RecoveryKey` will not
    /// refresh an already-consumed grant (caller should escalate via guidance).
    pub fn grant_evidence_read(&mut self, path: &Path, key: RecoveryKey) -> bool {
        if self.evidence_grant_key.as_ref() == Some(&key)
            && self.evidence_grant_remaining == 0
            && self
                .evidence_grant_path
                .as_ref()
                .is_some_and(|p| paths_equal(p, path))
        {
            // Already used for this key+path — no unlimited refresh.
            return false;
        }
        self.evidence_grant_path = Some(path.to_path_buf());
        self.evidence_grant_remaining = 1;
        self.evidence_grant_key = Some(key);
        true
    }

    /// Consume the MissingEvidence one-shot grant (scope allow + dup-read bypass).
    pub fn try_consume_evidence_grant(&mut self, path: &Path) -> bool {
        if !self.has_evidence_grant_for(path) {
            return false;
        }
        self.evidence_grant_remaining = self.evidence_grant_remaining.saturating_sub(1);
        true
    }

    /// True when either FailedPatch or MissingEvidence grants this path.
    pub fn has_any_read_grant_for(&self, path: &Path) -> bool {
        self.has_grant_for(path) || self.has_evidence_grant_for(path)
    }

    /// Consume FailedPatch grant first, else MissingEvidence grant.
    pub fn try_consume_any_read_grant(&mut self, path: &Path) -> bool {
        self.try_consume_read(path) || self.try_consume_evidence_grant(path)
    }

    /// After a failed `apply_patch`: activate recovery, grant one reread, attach
    /// a grounded nearby-lines packet, and maybe terminate when the same-target
    /// failure budget is exhausted.
    pub fn after_patch_failure(&mut self, path: &Path, result: ToolResult) -> PatchFailureOutcome {
        self.after_patch_failure_with_packet(path, result, None)
    }

    /// Like [`after_patch_failure`], storing an optional grounded excerpt for
    /// the next retry (Slice 5G).
    pub fn after_patch_failure_with_packet(
        &mut self,
        path: &Path,
        mut result: ToolResult,
        packet: Option<GroundedPatchPacket>,
    ) -> PatchFailureOutcome {
        let path_buf = path.to_path_buf();
        let same_target = self.target.as_ref().is_some_and(|p| paths_equal(p, path));
        let show_phase_guidance = !(self.is_active() && same_target);

        if same_target && self.consecutive_failures > 0 {
            self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        } else {
            self.consecutive_failures = 1;
        }

        self.reason = Some(RecoveryReason::FailedPatch);
        self.target = Some(path_buf);
        self.remaining_reads = 1;

        if let Some(packet) = packet {
            self.grounded_lines = packet.lines;
            self.grounded_display = Some(packet.display);
        } else if !(same_target && !self.grounded_lines.is_empty()) {
            // New target / no packet: clear. Same-target retry without a fresh
            // excerpt (e.g. ungrounded reject) keeps the prior grounded lines.
            self.grounded_lines.clear();
            self.grounded_display = None;
        }

        let recovery = grounded_recovery_message(path, self.grounded_display.as_deref());
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

    /// True when FailedPatch is active for `path` with a non-empty grounded packet.
    pub fn has_grounded_packet_for(&self, path: &Path) -> bool {
        self.is_active()
            && self.target.as_ref().is_some_and(|t| paths_equal(t, path))
            && !self.grounded_lines.is_empty()
    }

    pub fn grounded_lines(&self) -> &[String] {
        &self.grounded_lines
    }

    /// Pre-dispatch gate (Slice 5G): after a mismatch, the next patch on the
    /// recovery target must cite ≥1 exact line from the grounded excerpt as
    /// context (` `) or removal (`-`). Counts against the FailedPatch budget
    /// here because the reject is pre-dispatch (Layer 4 would otherwise treat
    /// it as InvalidArguments and not increment `consecutive_failures`).
    pub fn reject_ungrounded_patch(
        &mut self,
        path: &Path,
        diff: &str,
    ) -> Option<ToolResult> {
        if !self.has_grounded_packet_for(path) {
            return None;
        }
        if patch_cites_grounded_line(diff, &self.grounded_lines) {
            return None;
        }

        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        self.remaining_reads = self.remaining_reads.max(1);

        let mut message = format!(
            "apply_patch rejected: hunk is not grounded in the recovery excerpt for `{}`.\n\
             Next patch MUST include at least one exact line from the excerpt below as \
             context or a removal line (small hunk, no invented surroundings).\n",
            path.display()
        );
        if let Some(display) = &self.grounded_display {
            message.push('\n');
            message.push_str(display);
            message.push('\n');
        }

        let mut result = ToolResult::error(message);
        if self.consecutive_failures >= MAX_CONSECUTIVE_PATCH_FAILURES {
            let stop = format!(
                "Stopped: {MAX_CONSECUTIVE_PATCH_FAILURES} consecutive apply_patch failures on {} \
                 without a successful edit. Regenerate from a fresh read or finish with blockers.",
                path.display()
            );
            if let Some(e) = &mut result.error {
                e.push_str("\n\n");
                e.push_str(&stop);
            }
            result.stop_reason = Some(StopReason::NoProgress);
        }
        Some(result)
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

/// Exact nearby file lines shown after a FailedPatch mismatch (Slice 5G).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroundedPatchPacket {
    pub lines: Vec<String>,
    pub display: String,
}

const GROUNDED_FALLBACK_LINES: usize = 24;

/// Build a grounded packet from the patch-failure error (preferred) or a
/// fresh file read fallback.
pub fn build_grounded_patch_packet(
    workspace_root: &Path,
    path: &Path,
    prior_error: Option<&str>,
) -> Option<GroundedPatchPacket> {
    if let Some(err) = prior_error {
        if let Some(packet) = packet_from_error_nearby_lines(err) {
            return Some(packet);
        }
    }
    packet_from_file_fallback(workspace_root, path)
}

fn packet_from_error_nearby_lines(err: &str) -> Option<GroundedPatchPacket> {
    let mut lines = Vec::new();
    let mut display = String::new();
    let mut in_nearby = false;
    for line in err.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("Actual nearby lines") {
            in_nearby = true;
            display.push_str(trimmed);
            display.push('\n');
            continue;
        }
        if !in_nearby {
            continue;
        }
        if trimmed.is_empty()
            || trimmed.starts_with("Regenerate")
            || trimmed.starts_with("Expected context")
            || trimmed.starts_with("Identifiers in the patch")
            || trimmed.starts_with("Patch failed")
            || trimmed.starts_with("Retry with")
        {
            break;
        }
        if let Some((num, rest)) = split_numbered_line(trimmed) {
            let body = rest.strip_prefix(' ').unwrap_or(rest);
            lines.push(body.to_string());
            display.push_str(&format!("{num} | {body}\n"));
        }
    }
    if lines.is_empty() {
        return None;
    }
    Some(GroundedPatchPacket { lines, display })
}

fn split_numbered_line(line: &str) -> Option<(&str, &str)> {
    let (num, rest) = line.split_once('|')?;
    let num = num.trim();
    if num.is_empty() || !num.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some((num, rest))
}

fn packet_from_file_fallback(workspace_root: &Path, path: &Path) -> Option<GroundedPatchPacket> {
    let content = std::fs::read_to_string(workspace_root.join(path)).ok()?;
    let file_lines: Vec<&str> = content.lines().collect();
    if file_lines.is_empty() {
        return None;
    }
    let take = GROUNDED_FALLBACK_LINES.min(file_lines.len());
    let mut lines = Vec::with_capacity(take);
    let mut display = format!("Actual nearby lines 1–{take}:\n");
    for (i, line) in file_lines.iter().take(take).enumerate() {
        lines.push((*line).to_string());
        display.push_str(&format!("{} | {}\n", i + 1, line));
    }
    Some(GroundedPatchPacket { lines, display })
}

fn grounded_recovery_message(path: &Path, display: Option<&str>) -> String {
    let mut msg = format!(
        "\n\nPatch failed for `{}` because the expected context did not match (or made no change).\n",
        path.display()
    );
    if let Some(display) = display {
        msg.push_str(display);
        if !display.ends_with('\n') {
            msg.push('\n');
        }
    }
    msg.push_str(
        "\nRetry with:\n\
         - a small hunk;\n\
         - exact existing context from the excerpt above;\n\
         - no invented surrounding code.\n\
         You may reread this file once (recovery grant), then apply_patch again.\n\
         Next apply_patch MUST include at least one exact line from the excerpt as \
         context or a removal. Do not use write_file to replace the whole file.",
    );
    msg
}

/// True when the unified diff cites ≥1 exact grounded line as context or removal.
pub fn patch_cites_grounded_line(diff: &str, grounded: &[String]) -> bool {
    if grounded.is_empty() {
        return false;
    }
    let grounded_trimmed: Vec<&str> = grounded.iter().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
    if grounded_trimmed.is_empty() {
        return false;
    }
    for line in extract_patch_anchor_lines(diff) {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if grounded_trimmed.iter().any(|g| *g == t) {
            return true;
        }
    }
    false
}

/// Context (` `) and removal (`-`) bodies from a unified diff.
fn extract_patch_anchor_lines(patch: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_hunk = false;
    for line in patch.lines() {
        if line.starts_with("@@") {
            in_hunk = true;
            continue;
        }
        if line.starts_with("---") || line.starts_with("+++") {
            continue;
        }
        if !in_hunk {
            if line.starts_with(' ') || line.starts_with('-') || line.starts_with('+') {
                in_hunk = true;
            } else {
                continue;
            }
        }
        if line.starts_with('+') && !line.starts_with("+++") {
            continue;
        }
        if let Some(rest) = line.strip_prefix('-').filter(|r| !r.starts_with('-')) {
            out.push(rest.to_string());
        } else if let Some(rest) = line.strip_prefix(' ') {
            out.push(rest.to_string());
        }
    }
    out
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

    #[test]
    fn guidance_attempts_are_keyed_by_reason_tool_target() {
        let mut s = RecoveryState::new();
        let a = RecoveryKey::new("invalid_arguments", "apply_patch", Some(PathBuf::from("a.rs")));
        let a2 = RecoveryKey::new("invalid_arguments", "apply_patch", Some(PathBuf::from("a.rs")));
        let b = RecoveryKey::new("missing_evidence", "read_file", Some(PathBuf::from("b.rs")));
        let a_other =
            RecoveryKey::new("invalid_arguments", "apply_patch", Some(PathBuf::from("c.rs")));

        assert_eq!(s.note_guidance_attempt(a.clone()), 1);
        assert_eq!(s.note_guidance_attempt(a2), 2);
        // Different reason → new sequence.
        assert_eq!(s.note_guidance_attempt(b), 1);
        // Different target under same reason → new sequence.
        assert_eq!(s.note_guidance_attempt(a_other), 1);
        // Back to original key → new sequence (not resume).
        assert_eq!(s.note_guidance_attempt(a), 1);
    }

    #[test]
    fn evidence_grant_is_one_shot_and_does_not_activate_failed_patch_mode() {
        let mut s = RecoveryState::new();
        let path = Path::new("src/helper.ts");
        let key = RecoveryKey::new("missing_evidence", "read_file", Some(path.to_path_buf()));
        assert!(s.grant_evidence_read(path, key.clone()));
        assert!(!s.is_active());
        assert!(s.has_evidence_grant_for(path));
        assert!(s.try_consume_evidence_grant(path));
        assert!(!s.has_evidence_grant_for(path));
        // Same key cannot refresh after consume.
        assert!(!s.grant_evidence_read(path, key));
    }

    #[test]
    fn act_duplicate_escalation_is_per_path_and_clears_without_touching_patch_budget() {
        let mut s = RecoveryState::new();
        let a = Path::new("Board.tsx");
        let b = Path::new("store.ts");
        assert_eq!(s.note_act_duplicate(a), 1);
        assert!(s.is_act_duplicate_restricted(a));
        assert!(!s.is_act_duplicate_path_disabled(a));
        assert_eq!(s.note_act_duplicate(a), 2);
        assert!(s.is_act_duplicate_path_disabled(a));
        assert_eq!(s.note_act_duplicate(b), 1);
        assert!(!s.is_act_duplicate_path_disabled(b));

        let _ = s.after_patch_failure(Path::new("x.rs"), ToolResult::error("hunk"));
        assert_eq!(s.consecutive_failures(), 1);
        s.clear_duplicate_escalation();
        assert_eq!(s.duplicate_escalation_level(a), 0);
        assert_eq!(s.consecutive_failures(), 1);
        assert!(s.is_active());
    }

    #[test]
    fn part5g_packet_from_error_and_grounded_retry_gate() {
        let err = "Patch hunk did not match views.py.\n\
                   Engine: No patches were found\n\
                   \n\
                   Actual nearby lines 30–33:\n\
                   30 |     def get(self, request: Request) -> Response:\n\
                   31 |         images = AnnotationImage.objects.filter(user=request.user).order_by(\n\
                   32 |             \"-uploaded_at\"\n\
                   33 |         )\n\
                   \n\
                   Regenerate the patch using the actual file contents.";
        let packet = packet_from_error_nearby_lines(err).expect("packet");
        assert!(packet.lines.iter().any(|l| l.contains("AnnotationImage.objects.filter")));
        assert!(packet.display.contains("30 |"));

        let mut s = RecoveryState::new();
        let path = Path::new("backend/annotations/views.py");
        let out = s.after_patch_failure_with_packet(
            path,
            ToolResult::error(err),
            Some(packet),
        );
        assert!(out.result.error.as_deref().unwrap().contains("Retry with"));
        assert!(out.result.error.as_deref().unwrap().contains("MUST include"));
        assert_eq!(s.consecutive_failures(), 1);
        assert!(s.has_grounded_packet_for(path));

        // Invented surrounding / empty context — rejected and budget ++.
        let bad = "--- a/views.py\n+++ b/views.py\n@@ -1,3 +1,3 @@\n-invented_line_not_in_excerpt()\n+prefetch_related(\"shapes\")\n";
        let denied = s.reject_ungrounded_patch(path, bad).expect("ungrounded");
        assert!(denied.error.as_deref().unwrap().contains("not grounded"));
        assert_eq!(s.consecutive_failures(), 2);

        // Exact line from excerpt as removal — allowed through the gate.
        let good = "--- a/views.py\n+++ b/views.py\n@@\n-        images = AnnotationImage.objects.filter(user=request.user).order_by(\n+        images = AnnotationImage.objects.filter(user=request.user).prefetch_related(\"shapes\").order_by(\n";
        assert!(s.reject_ungrounded_patch(path, good).is_none());
        assert_eq!(s.consecutive_failures(), 2);
    }

    #[test]
    fn part5g_file_fallback_packet_when_error_has_no_nearby_block() {
        let dir = tempdir().unwrap();
        let rel = Path::new("views.py");
        fs::write(
            dir.path().join(rel),
            "line_one\nline_two\nline_three\nline_four\n",
        )
        .unwrap();
        let packet = build_grounded_patch_packet(dir.path(), rel, Some("hunk mismatch only"))
            .expect("fallback");
        assert!(packet.lines.iter().any(|l| l == "line_one"));
        assert!(packet.display.contains("1 | line_one"));
    }

    #[test]
    fn part5g_third_ungrounded_reject_stops() {
        let mut s = RecoveryState::new();
        let path = Path::new("a.py");
        let packet = GroundedPatchPacket {
            lines: vec!["    x = 1".into()],
            display: "1 |     x = 1\n".into(),
        };
        let _ = s.after_patch_failure_with_packet(path, ToolResult::error("fail"), Some(packet));
        assert_eq!(s.consecutive_failures(), 1);
        let bad = "--- a\n+++ b\n@@\n-not_the_line\n+y = 2\n";
        assert!(s.reject_ungrounded_patch(path, bad).unwrap().stop_reason.is_none());
        assert_eq!(s.consecutive_failures(), 2);
        let stop = s.reject_ungrounded_patch(path, bad).unwrap();
        assert_eq!(stop.stop_reason, Some(StopReason::NoProgress));
        assert_eq!(s.consecutive_failures(), 3);
    }
}
