//! Runtime phase control for the agent loop (slice 1–6).
//!
//! The model still chooses actions, but the runtime owns which phase the run is
//! in. Tool schemas are filtered from [`Phase`] / recovery mode (slice 2–4).
//! Explore → Act is driven by progress / evidence-checkpoint signals (slice 3).
//! Failed `apply_patch` enters a sticky recovery tool set (slice 4).
//! Successful edits enter Verify; pass / unusable verify enter Finish (slice 5).
//! Consecutive harness denials terminate with [`StopReason::NoProgress`] (slice 6).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::state::recovery::RecoveryState;
use crate::state::step::Action;
use crate::state::stop_reason::StopReason;
use crate::tools::ToolResult;
use crate::verification::{VerificationOutcome, VerificationStatus};

/// Consecutive no-progress observations that force Explore → Act.
pub const NO_PROGRESS_ACT_THRESHOLD: usize = 2;

/// Consecutive harness denials that stop the run as [`StopReason::NoProgress`].
///
/// Kept well below typical `MAX_STEPS` so denial spirals cannot burn the budget.
pub const NO_PROGRESS_STOP_THRESHOLD: usize = 6;

/// Extra model turns allowed in Finish before harness auto-completes.
pub const FINISH_PHASE_IDLE_TURNS: u32 = 1;

/// Message injected when Explore → Act (capability change is the real control).
pub const ACT_TRANSITION_MESSAGE: &str = "\
Exploration is complete. Use the evidence already collected.
You must now update_plan, apply_patch, or finish with a concrete code blocker. \
Do not read or search further. Do not finish by asking for more tools or permissions.";

/// Sticky reminder while Act remains open without an edit attempt.
pub const ACT_STICKY_MESSAGE: &str = "\
Act phase: tools are update_plan, apply_patch, finish, and read_file \
(already-seen files only — re-reading them wastes a turn). \
Do not search or request unseen files. \
finish must name a concrete code-level blocker — never ask for tool permissions.";

/// Message when a failed patch locks tools to reread → patch / finish.
pub const PATCH_RECOVERY_MESSAGE: &str = "\
Patch failed. Do not reopen exploration.
Reread the failed file once if needed, then submit a smaller apply_patch \
or finish with a blocker.";

/// Message when a successful edit enters Verify.
pub const VERIFY_TRANSITION_MESSAGE: &str = "\
Edit applied. Verify with a focused check (or git_status/git_diff), \
re-edit if needed, or finish.";

/// Message when verification passed — finish-only tools.
pub const FINISH_VERIFIED_MESSAGE: &str = "\
Verification passed. Call finish with a short summary. \
No further exploration or edits.";

/// Message when verify is unusable (sandbox/exit 133) — finish unverified.
pub const FINISH_UNVERIFIED_MESSAGE: &str = "\
Verification could not run meaningfully in this environment. \
Do not chase the same broken check. Call finish summarizing the edit \
(unverified).";

/// High-level loop phase. Tool availability is derived from this (slice 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Phase {
    #[default]
    Explore,
    Act,
    Verify,
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

/// Act phase: plan / patch / finish, plus `read_file` scoped to already-seen
/// files. No search or shell.
///
/// `read_file` was previously removed from this list entirely after eval runs
/// showed models thrashing on duplicate reads instead of editing. It is back
/// in the schema (`final-design-plan.md` §5.1, §8 Slice 1): the tool no longer
/// disappears out from under the model, but `action_policy::classify_invalid`
/// still rejects reads of files outside `RunState.seen_files` via a bounded
/// `ToolScope`, exactly as before minus the schema cliff. Patch-recovery
/// separately grants one targeted read of the failed path regardless of
/// whether it was previously seen.
pub const ACT_TOOLS: &[&str] = &["read_file", "update_plan", "apply_patch", "finish"];

/// After a failed patch: reread target → patch / finish only (slice 4).
pub const PATCH_RECOVERY_TOOLS: &[&str] = &["read_file", "apply_patch", "finish"];

/// Verify: inspect + re-edit + finish (slice 5). No broad search.
pub const VERIFY_TOOLS: &[&str] = &[
    "read_file",
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
    /// Model turn when Finish phase began (slice 5 auto-complete).
    pub entered_finish_turn: Option<u32>,
    /// Consecutive harness denials (phase / checkpoint / duplicate / blocked).
    pub consecutive_denials: usize,
    /// Consecutive invalid actions under the phase contract (separate from denials).
    pub consecutive_invalid_actions: usize,
    /// Next LLM request should prefer `tool_choice=required` when the provider allows it.
    pub require_tool_choice: bool,
    /// One-shot guidance queued for the next model turn (injected into goal).
    pub pending_phase_guidance: Option<String>,
    /// While true in Act, re-inject a sticky edit reminder each turn.
    pub act_force_edit: bool,
}

impl RunState {
    pub fn new() -> Self {
        Self {
            phase: Phase::Explore,
            seen_files: HashSet::new(),
            seen_searches: HashSet::new(),
            no_progress: 0,
            justified_unseen_reads_left: 1,
            entered_finish_turn: None,
            consecutive_denials: 0,
            consecutive_invalid_actions: 0,
            require_tool_choice: false,
            pending_phase_guidance: None,
            act_force_edit: false,
        }
    }

    /// Reset the invalid-action ladder after a valid phase action.
    pub fn note_valid_action(&mut self) {
        self.consecutive_invalid_actions = 0;
        self.require_tool_choice = false;
    }

    pub fn is_explore(&self) -> bool {
        matches!(self.phase, Phase::Explore)
    }

    pub fn is_act(&self) -> bool {
        matches!(self.phase, Phase::Act)
    }

    pub fn is_verify(&self) -> bool {
        matches!(self.phase, Phase::Verify)
    }

    pub fn is_finish(&self) -> bool {
        matches!(self.phase, Phase::Finish)
    }

    /// Tool names exposed for the current phase / recovery mode.
    ///
    /// Recovery tool lock lives on [`RecoveryState`] (Slice 4 Part 1), not here.
    pub fn effective_tools(&self, recovery: &RecoveryState) -> &'static [&'static str] {
        if recovery.is_active() {
            PATCH_RECOVERY_TOOLS
        } else {
            self.phase.allowed_tools()
        }
    }

    /// Label for logs / deny messages (`explore` / `act` / `patch_recovery` / …).
    pub fn tool_mode_label(&self, recovery: &RecoveryState) -> &'static str {
        if recovery.is_active() {
            "patch_recovery"
        } else {
            self.phase.as_str()
        }
    }

    /// Record that a file was successfully read into context.
    pub fn note_file_seen(&mut self, path: PathBuf) {
        self.seen_files.insert(path);
    }

    pub fn has_seen_file(&self, path: &Path) -> bool {
        self.seen_files.iter().any(|p| paths_equal(p, path))
    }

    /// Record a search signature. Returns `true` if this signature is new.
    pub fn note_search(&mut self, signature: impl Into<String>) -> bool {
        self.seen_searches.insert(signature.into())
    }

    /// Whether Act / Verify / recovery may read this path.
    ///
    /// Recovery: only the failed patch path.
    /// Act / Verify: seen paths, or one justified unseen (Act only for justified).
    pub fn allow_act_read(
        &mut self,
        recovery: &RecoveryState,
        path: &Path,
    ) -> Result<(), String> {
        if recovery.is_active() {
            if recovery
                .target()
                .is_some_and(|p| paths_equal(p, path))
            {
                return Ok(());
            }
            let target = recovery
                .target()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "<unknown>".to_string());
            return Err(format!(
                "Patch recovery: read_file is limited to `{target}`. \
                 Then apply_patch or finish with a blocker."
            ));
        }

        if self.has_seen_file(path) {
            return Ok(());
        }
        if matches!(self.phase, Phase::Act) && self.justified_unseen_reads_left > 0 {
            self.justified_unseen_reads_left -= 1;
            self.note_file_seen(path.to_path_buf());
            return Ok(());
        }
        Err(
            "read_file is limited to files already in context. \
             Update the plan, apply a patch, run a check, or finish."
                .to_string(),
        )
    }

    /// Enter Act if still exploring. Returns `true` when the phase changed.
    /// Never reopens Explore once left (recovery / Act stay closed).
    pub fn enter_act(&mut self) -> bool {
        if matches!(self.phase, Phase::Explore) {
            self.phase = Phase::Act;
            self.justified_unseen_reads_left = 0;
            self.act_force_edit = true;
            let msg = self.act_guidance_with_evidence(ACT_TRANSITION_MESSAGE);
            self.queue_phase_guidance(&msg);
            true
        } else {
            false
        }
    }

    pub fn queue_phase_guidance(&mut self, message: &str) {
        self.pending_phase_guidance = Some(message.to_string());
    }

    /// Guidance to inject into the next LLM goal (one-shot + sticky Act reminder).
    pub fn phase_guidance_for_injection(
        &mut self,
        recovery: &RecoveryState,
    ) -> Option<String> {
        if let Some(msg) = self.pending_phase_guidance.take() {
            return Some(msg);
        }
        if self.act_force_edit
            && matches!(self.phase, Phase::Act)
            && !recovery.is_active()
        {
            return Some(self.act_guidance_with_evidence_and_recovery(ACT_STICKY_MESSAGE, recovery));
        }
        None
    }

    /// Append seen-file evidence so Act pressure names concrete patch targets.
    pub fn act_guidance_with_evidence(&self, base: &str) -> String {
        let mut out = base.to_string();
        if self.seen_files.is_empty() {
            return out;
        }
        let mut paths: Vec<_> = self.seen_files.iter().collect();
        paths.sort_by_key(|p| p.to_string_lossy().to_string());
        out.push_str("\n\nFiles already in context (patch from these — do not re-read):\n");
        for path in paths.iter().take(12) {
            out.push_str(&format!("- {}\n", path.display()));
        }
        out.push_str(
            "Call apply_patch on the likely fix now. \
             finish may only report a concrete code blocker (file + bug), \
             never request more tools or permissions.",
        );
        out
    }

    /// Sticky Act reminder including Part 5B duplicate-path restrictions.
    pub fn act_guidance_with_evidence_and_recovery(
        &self,
        base: &str,
        recovery: &RecoveryState,
    ) -> String {
        let mut out = self.act_guidance_with_evidence(base);
        let blocked = recovery.duplicate_escalation_paths();
        if blocked.is_empty() {
            return out;
        }
        out.push_str("\n\nDuplicate-evidence restrictions (read_file still in schema):\n");
        for (path, level) in blocked {
            if level >= 2 {
                out.push_str(&format!(
                    "- {} — read_file DISABLED for this path\n",
                    path.display()
                ));
            } else {
                out.push_str(&format!(
                    "- {} — duplicate/cached ranges blocked\n",
                    path.display()
                ));
            }
        }
        out
    }

    /// Clear Act "must edit" pressure after a successful workspace mutation.
    ///
    /// `update_plan` alone must not clear this — otherwise the model plans,
    /// drops sticky pressure, then finishes asking for more tools.
    pub fn note_act_progress(&mut self) {
        self.act_force_edit = false;
        self.pending_phase_guidance = None;
    }

    /// Phase-side effects when patch recovery engages: lock into Act and queue
    /// the recovery guidance message. Recovery bookkeeping (`reason` / `target`
    /// / failure budgets) lives on [`RecoveryState`] — call this only when
    /// `RecoveryState::after_patch_failure` reports `show_phase_guidance`.
    pub fn engage_patch_recovery_phase(&mut self) -> Option<&'static str> {
        let _ = self.enter_act();
        if matches!(self.phase, Phase::Verify | Phase::Finish) {
            self.phase = Phase::Act;
        }
        self.entered_finish_turn = None;
        self.act_force_edit = false;
        self.queue_phase_guidance(PATCH_RECOVERY_MESSAGE);
        Some(PATCH_RECOVERY_MESSAGE)
    }

    /// Enter Verify after a successful workspace mutation (slice 5).
    pub fn enter_verify(&mut self, recovery: &RecoveryState) -> Option<&'static str> {
        if recovery.is_active() {
            return None;
        }
        if matches!(self.phase, Phase::Verify) {
            return None;
        }
        self.phase = Phase::Verify;
        self.entered_finish_turn = None;
        self.act_force_edit = false;
        self.queue_phase_guidance(VERIFY_TRANSITION_MESSAGE);
        Some(VERIFY_TRANSITION_MESSAGE)
    }

    /// Enter Finish-only tools (slice 5).
    pub fn enter_finish(
        &mut self,
        recovery: &mut RecoveryState,
        model_turn: u32,
        verified: bool,
    ) -> Option<&'static str> {
        recovery.clear();
        let msg = if verified {
            FINISH_VERIFIED_MESSAGE
        } else {
            FINISH_UNVERIFIED_MESSAGE
        };
        if matches!(self.phase, Phase::Finish) {
            return None;
        }
        self.phase = Phase::Finish;
        self.entered_finish_turn = Some(model_turn);
        self.act_force_edit = false;
        self.queue_phase_guidance(msg);
        Some(msg)
    }

    /// React to a classified verification outcome (slice 5).
    pub fn observe_verification(
        &mut self,
        recovery: &mut RecoveryState,
        outcome: &VerificationOutcome,
        model_turn: u32,
    ) -> Option<&'static str> {
        match outcome.status {
            VerificationStatus::Passed => self.enter_finish(recovery, model_turn, true),
            VerificationStatus::EnvironmentBlocked => {
                self.enter_finish(recovery, model_turn, false)
            }
            VerificationStatus::Failed => {
                // Stay able to re-edit; ensure Verify tools (not Finish).
                if matches!(self.phase, Phase::Finish | Phase::Explore) {
                    self.enter_verify(recovery)
                } else {
                    None
                }
            }
        }
    }

    /// Auto-complete once the model has had a turn in Finish without calling finish.
    pub fn should_auto_complete_finish(&self, current_turn: u32) -> bool {
        match self.entered_finish_turn {
            Some(entered) => {
                current_turn.saturating_sub(entered) >= FINISH_PHASE_IDLE_TURNS
            }
            None => false,
        }
    }

    /// Update the denial streak from a tool outcome (slice 6).
    ///
    /// Returns [`StopReason::NoProgress`] when consecutive harness denials hit
    /// [`NO_PROGRESS_STOP_THRESHOLD`]. Any non-denial outcome clears the streak.
    ///
    /// Callers may demote the returned stop for committed `DuplicateEvidence`
    /// reads (Part 5E): the counter still increments for telemetry, but Layer 4
    /// owns terminate for duplicate/disabled-read thrash.
    pub fn observe_denial_streak(&mut self, result: &ToolResult) -> Option<StopReason> {
        if is_denied_result(result) {
            self.consecutive_denials = self.consecutive_denials.saturating_add(1);
            if self.consecutive_denials >= NO_PROGRESS_STOP_THRESHOLD {
                return Some(StopReason::NoProgress);
            }
            None
        } else {
            self.consecutive_denials = 0;
            None
        }
    }

    /// Message appended when denial-streak stop fires.
    pub fn no_progress_stop_message(&self, recovery: &RecoveryState) -> String {
        format!(
            "Stopped: {NO_PROGRESS_STOP_THRESHOLD} consecutive harness denials \
             (phase/checkpoint/duplicate/blocked) with no productive action. \
             Last mode: {}.",
            self.tool_mode_label(recovery)
        )
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
            // Still track seen files while in Act / recovery for read gating.
            self.record_side_effects(action, result, duplicate_read);
            return None;
        }

        let progressed = self.classify_progress(action, result, duplicate_read);
        match progressed {
            ProgressSignal::Progress => self.no_progress = 0,
            ProgressSignal::NoProgress => {
                self.no_progress = self.no_progress.saturating_add(1);
            }
            ProgressSignal::Neutral => {}
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

    /// Classify whether this outcome advances Explore evidence.
    fn classify_progress(
        &mut self,
        action: &Action,
        result: &ToolResult,
        duplicate_read: bool,
    ) -> ProgressSignal {
        if is_denied_result(result) || duplicate_read {
            self.record_side_effects(action, result, duplicate_read);
            return ProgressSignal::NoProgress;
        }

        match action {
            Action::ReadFile { path, .. } => {
                if !result.success {
                    return ProgressSignal::Neutral;
                }
                let path = path.clone();
                let is_new = !self.has_seen_file(&path);
                self.note_file_seen(path);
                if is_new {
                    ProgressSignal::Progress
                } else {
                    ProgressSignal::NoProgress
                }
            }
            Action::SearchCode { query, path } | Action::Locate { query, path } => {
                if !result.success {
                    return ProgressSignal::Neutral;
                }
                let sig = search_signature(action, query, path);
                if self.note_search(sig) {
                    ProgressSignal::Progress
                } else {
                    ProgressSignal::NoProgress
                }
            }
            Action::FindFiles { pattern, path } => {
                if !result.success {
                    return ProgressSignal::Neutral;
                }
                let sig = format!("find_files:{pattern}:@{}", path.display());
                if self.note_search(sig) {
                    ProgressSignal::Progress
                } else {
                    ProgressSignal::NoProgress
                }
            }
            Action::UpdatePlan { .. } if result.success => ProgressSignal::Progress,
            Action::ApplyPatch { .. } | Action::WriteFile { .. } if result.success => {
                ProgressSignal::Progress
            }
            Action::RunCommand { cmd, .. } | Action::RunShell { command: cmd, .. } => {
                // Shell that only dumps an already-seen file = no progress.
                if let Some(intent) = crate::tools::shell_read::detect_shell_file_read(cmd) {
                    if self.has_seen_file(&intent.path) {
                        return ProgressSignal::NoProgress;
                    }
                    // First dump of a new path counts as seeing that file.
                    if result.success {
                        self.note_file_seen(intent.path);
                        return ProgressSignal::Progress;
                    }
                    return ProgressSignal::Neutral;
                }
                // Other shell (verify / git / build): success = progress;
                // failure is not Explore thrash — leave the streak alone.
                if result.success {
                    ProgressSignal::Progress
                } else {
                    ProgressSignal::Neutral
                }
            }
            Action::Finish { .. } => ProgressSignal::Progress,
            _ => {
                if result.success {
                    ProgressSignal::Progress
                } else {
                    ProgressSignal::Neutral
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProgressSignal {
    Progress,
    NoProgress,
    /// Failed non-thrash actions (e.g. broken verify) — do not force Act.
    Neutral,
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
        return false; // failed attempt without denial text — not a harness denial
    };
    // Invalid-action ladder owns ACTION_NOT_AVAILABLE; do not double-count toward
    // the general denial-streak stop.
    if err.starts_with("ACTION_NOT_AVAILABLE")
        || err.starts_with("PHASE TRANSITION: ACT")
        || err.starts_with("Stopped after ")
    {
        return false;
    }
    let l = err.to_ascii_lowercase();
    l.contains("duplicate read denied")
        || l.contains("exact content already available")
        || l.contains("exploration checkpoint active")
        || l.contains("is not available in")
        || l.contains("phase_tool_denied")
        || l.contains("act phase: read_file")
        || l.contains("read_file is limited")
        || l.contains("patch recovery:")
        || l.contains("blocked:")
        || l.contains("explore-shell budget")
}

fn paths_equal(a: &Path, b: &Path) -> bool {
    normalize_path(a) == normalize_path(b)
}

fn normalize_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .trim_start_matches("./")
        .to_string()
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
        let recovery = RecoveryState::new();
        state.phase = Phase::Act;
        let known = PathBuf::from("a.rs");
        state.note_file_seen(known.clone());
        assert!(state.allow_act_read(&recovery, &known).is_ok());

        let first_new = PathBuf::from("b.rs");
        assert!(state.allow_act_read(&recovery, &first_new).is_ok());
        assert_eq!(state.justified_unseen_reads_left, 0);
        assert!(state.has_seen_file(&first_new));

        let second_new = PathBuf::from("c.rs");
        assert!(state.allow_act_read(&recovery, &second_new).is_err());
    }

    #[test]
    fn act_tools_exclude_search_and_shell_but_allow_scoped_read() {
        assert!(!Phase::Act.allows_tool("search_code"));
        assert!(!Phase::Act.allows_tool("run_shell"));
        assert!(!Phase::Act.allows_tool("run_command"));
        assert!(!Phase::Act.allows_tool("locate"));
        // read_file is visible in the schema (Slice 1) — see
        // action_policy::classify_invalid for the seen-files scope that
        // still rejects unseen-path reads.
        assert!(Phase::Act.allows_tool("read_file"));
        assert!(Phase::Act.allows_tool("apply_patch"));
        assert!(Phase::Act.allows_tool("update_plan"));
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
        let recovery = RecoveryState::new();
        let msg = state.observe_tool_outcome(&read("fresh.rs"), &ok_result(), false, true);
        assert_eq!(msg, Some(ACT_TRANSITION_MESSAGE));
        assert_eq!(state.phase, Phase::Act);
        assert!(state.act_force_edit);
        assert!(state
            .phase_guidance_for_injection(&recovery)
            .unwrap()
            .contains("Exploration is complete"));
    }

    #[test]
    fn enter_act_queues_guidance_and_forces_edit() {
        let mut state = RunState::new();
        let recovery = RecoveryState::new();
        state.note_file_seen(PathBuf::from("Board.tsx"));
        assert!(state.enter_act());
        assert!(state.act_force_edit);
        let g = state.phase_guidance_for_injection(&recovery).unwrap();
        assert!(g.contains("Exploration is complete"));
        assert!(g.contains("Board.tsx"));
        // Sticky reminder on subsequent turns still lists evidence.
        let sticky = state.phase_guidance_for_injection(&recovery).unwrap();
        assert!(sticky.contains("Act phase"));
        assert!(sticky.contains("Board.tsx"));
        state.note_act_progress();
        assert!(state.phase_guidance_for_injection(&recovery).is_none());
    }

    #[test]
    fn sticky_act_guidance_lists_duplicate_path_restrictions() {
        let mut state = RunState::new();
        let mut recovery = RecoveryState::new();
        state.note_file_seen(PathBuf::from("Board.tsx"));
        assert!(state.enter_act());
        let _ = state.phase_guidance_for_injection(&recovery); // drain one-shot
        let _ = recovery.note_act_duplicate(Path::new("Board.tsx"));
        let sticky = state.phase_guidance_for_injection(&recovery).unwrap();
        assert!(sticky.contains("Duplicate-evidence restrictions"));
        assert!(sticky.contains("Board.tsx"));
        assert!(sticky.contains("duplicate/cached ranges blocked"));
        let _ = recovery.note_act_duplicate(Path::new("Board.tsx"));
        let sticky2 = state.phase_guidance_for_injection(&recovery).unwrap();
        assert!(sticky2.contains("DISABLED"));
    }

    #[test]
    fn failed_patch_enters_recovery_and_forces_act() {
        let mut state = RunState::new();
        let mut recovery = RecoveryState::new();
        assert_eq!(state.phase, Phase::Explore);
        let out = recovery.after_patch_failure(
            Path::new("src/Board.tsx"),
            ToolResult::error("hunk mismatch"),
        );
        assert!(out.show_phase_guidance);
        let msg = state.engage_patch_recovery_phase();
        assert_eq!(msg, Some(PATCH_RECOVERY_MESSAGE));
        assert_eq!(state.phase, Phase::Act);
        assert!(recovery.is_active());
        assert_eq!(
            state.effective_tools(&recovery),
            PATCH_RECOVERY_TOOLS
        );
        assert!(!state.effective_tools(&recovery).contains(&"update_plan"));
        assert!(!state.effective_tools(&recovery).contains(&"search_code"));
        // Same path again: recovery stays active, no fresh phase guidance.
        let again = recovery.after_patch_failure(
            Path::new("src/Board.tsx"),
            ToolResult::error("hunk mismatch 2"),
        );
        assert!(!again.show_phase_guidance);
    }

    #[test]
    fn recovery_read_allows_only_failed_path() {
        let mut state = RunState::new();
        let mut recovery = RecoveryState::new();
        let _ = recovery.after_patch_failure(Path::new("a.rs"), ToolResult::error("fail"));
        let _ = state.engage_patch_recovery_phase();
        assert!(state.allow_act_read(&recovery, Path::new("a.rs")).is_ok());
        assert!(state.allow_act_read(&recovery, Path::new("b.rs")).is_err());
    }

    #[test]
    fn successful_clear_leaves_act_without_recovery() {
        let mut state = RunState::new();
        let mut recovery = RecoveryState::new();
        let _ = recovery.after_patch_failure(Path::new("a.rs"), ToolResult::error("fail"));
        let _ = state.engage_patch_recovery_phase();
        recovery.note_successful_mutation();
        assert!(!recovery.is_active());
        assert!(recovery.target().is_none());
        assert_eq!(state.phase, Phase::Act);
        assert_eq!(state.effective_tools(&recovery), ACT_TOOLS);
    }

    fn outcome(status: VerificationStatus) -> VerificationOutcome {
        VerificationOutcome {
            check_id: "root:test".into(),
            kind: crate::verification::CheckKind::Test,
            status,
            exit_code: Some(0),
            summary: "x".into(),
        }
    }

    #[test]
    fn successful_edit_enters_verify() {
        let mut state = RunState::new();
        let recovery = RecoveryState::new();
        let msg = state.enter_verify(&recovery);
        assert_eq!(msg, Some(VERIFY_TRANSITION_MESSAGE));
        assert_eq!(state.phase, Phase::Verify);
        assert!(state.effective_tools(&recovery).contains(&"run_shell"));
        assert!(state.effective_tools(&recovery).contains(&"apply_patch"));
        assert!(!state.effective_tools(&recovery).contains(&"search_code"));
        assert!(!state.effective_tools(&recovery).contains(&"update_plan"));
    }

    #[test]
    fn passed_verify_enters_finish_only() {
        let mut state = RunState::new();
        let mut recovery = RecoveryState::new();
        let _ = state.enter_verify(&recovery);
        let msg = state.observe_verification(&mut recovery, &outcome(VerificationStatus::Passed), 3);
        assert_eq!(msg, Some(FINISH_VERIFIED_MESSAGE));
        assert_eq!(state.phase, Phase::Finish);
        assert_eq!(state.effective_tools(&recovery), FINISH_TOOLS);
        assert_eq!(state.entered_finish_turn, Some(3));
        assert!(!state.should_auto_complete_finish(3));
        assert!(state.should_auto_complete_finish(4));
    }

    #[test]
    fn env_blocked_verify_enters_finish_unverified() {
        let mut state = RunState::new();
        let mut recovery = RecoveryState::new();
        let _ = state.enter_verify(&recovery);
        let msg = state.observe_verification(
            &mut recovery,
            &outcome(VerificationStatus::EnvironmentBlocked),
            5,
        );
        assert_eq!(msg, Some(FINISH_UNVERIFIED_MESSAGE));
        assert_eq!(state.phase, Phase::Finish);
        assert_eq!(state.effective_tools(&recovery), FINISH_TOOLS);
    }

    #[test]
    fn failed_verify_stays_in_verify() {
        let mut state = RunState::new();
        let mut recovery = RecoveryState::new();
        let _ = state.enter_verify(&recovery);
        let msg =
            state.observe_verification(&mut recovery, &outcome(VerificationStatus::Failed), 2);
        assert!(msg.is_none());
        assert_eq!(state.phase, Phase::Verify);
        assert!(state.effective_tools(&recovery).contains(&"apply_patch"));
        assert!(state.effective_tools(&recovery).contains(&"run_shell"));
    }

    fn denied_result(msg: &str) -> ToolResult {
        ToolResult {
            success: false,
            output: String::new(),
            diff: None,
            error: Some(msg.into()),
            stop_reason: None,
        }
    }

    #[test]
    fn six_consecutive_denials_stop_with_no_progress() {
        let mut state = RunState::new();
        let denied = denied_result(
            "Tool `search_code` is not available in act phase. Allowed: read_file, update_plan, apply_patch, finish.",
        );
        for i in 1..NO_PROGRESS_STOP_THRESHOLD {
            assert!(
                state.observe_denial_streak(&denied).is_none(),
                "should not stop at denial {i}"
            );
            assert_eq!(state.consecutive_denials, i);
        }
        assert_eq!(
            state.observe_denial_streak(&denied),
            Some(StopReason::NoProgress)
        );
        assert_eq!(state.consecutive_denials, NO_PROGRESS_STOP_THRESHOLD);
    }

    #[test]
    fn productive_outcome_clears_denial_streak() {
        let mut state = RunState::new();
        let denied = denied_result("Duplicate read denied: Exact content already available");
        for _ in 0..3 {
            let _ = state.observe_denial_streak(&denied);
        }
        assert_eq!(state.consecutive_denials, 3);
        assert!(state.observe_denial_streak(&ok_result()).is_none());
        assert_eq!(state.consecutive_denials, 0);
    }

    #[test]
    fn part5e_duplicate_denials_still_count_for_telemetry() {
        // Demotion of the stop is at the commit_tool_outcome call site;
        // observe_denial_streak itself still increments and would return stop.
        let mut state = RunState::new();
        let denied = denied_result("Duplicate read denied: Exact content already available");
        for i in 1..NO_PROGRESS_STOP_THRESHOLD {
            assert!(state.observe_denial_streak(&denied).is_none());
            assert_eq!(state.consecutive_denials, i);
        }
        assert_eq!(
            state.observe_denial_streak(&denied),
            Some(StopReason::NoProgress)
        );
        assert_eq!(state.consecutive_denials, NO_PROGRESS_STOP_THRESHOLD);
    }

    #[test]
    fn patch_failure_is_not_a_harness_denial() {
        let mut state = RunState::new();
        let denied = denied_result("Exploration checkpoint active (no-progress streak=5)");
        let _ = state.observe_denial_streak(&denied);
        assert_eq!(state.consecutive_denials, 1);

        let patch_fail = ToolResult {
            success: false,
            output: String::new(),
            diff: None,
            error: Some("hunk mismatch: context did not match".into()),
            stop_reason: None,
        };
        assert!(state.observe_denial_streak(&patch_fail).is_none());
        assert_eq!(state.consecutive_denials, 0);
    }
}
