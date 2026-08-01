//! Layer 4 runtime decision (final-design-plan.md §3, §6, §7.2, §8 Slice 4).
//!
//! **Part 2:** `Continue` / `Terminate` skeleton, 1:1 with `ToolResult.stop_reason`.
//! **Part 3:** `decide_next_state` may also return [`RuntimeTransition::Recover`]
//! for the five §7.2 recipes; [`apply_recovery`] mutates Layer 1 / result
//! guidance. Second application of the **same** [`RecoveryKey`] must not
//! produce byte-identical guidance (change-or-terminate).
//!
//! Finding 4 (shape-invalid `apply_patch`, never dispatched): classified as
//! [`RecoveryAction::InvalidArguments`], **not** [`RecoveryAction::FailedPatch`].
//! Failed-patch recovery (reread grant + failure budget) is only for patches
//! that actually dispatched and then failed against file content.
//!
//! **Part 3 checks:**
//! - [`RecoveryAction::MissingEvidence`] only for *relevant* unseen reads
//!   (recovery target, plan/goal/modified-file mention) — not every scope deny.
//! - Guidance attempt counts are keyed by [`RecoveryKey`], not the global
//!   stall counter.
//!
//! **Part 4:** exact/coarse fingerprint history escalates *general* stall
//! recipes. FailedPatch / verification / authorization budgets stay
//! authoritative on their own Layer 1 fields.

use std::path::{Path, PathBuf};

use crate::session::task_state::TaskSessionState;
use crate::state::agent_state::AgentState;
use crate::state::plan::AgentPlan;
use crate::state::recovery::RecoveryKey;
use crate::state::stall::StallKind;
use crate::state::stop_reason::StopReason;
use crate::tools::ToolResult;

/// Provisional Part 4: same-key general-recipe attempts at this count, with a
/// repeat/ping-pong fingerprint signal, terminate via Layer 4 (legacy
/// counters still dual-run).
const GENERAL_RECIPE_TERMINATE_ATTEMPTS: u32 = 3;

/// Result of applying a [`RecoveryAction`] to Layer 1 / the tool result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryApplyResult {
    pub stop_reason: Option<StopReason>,
    /// Phase guidance queued by FailedPatch (caller should `log_phase_transition`).
    pub phase_guidance: Option<&'static str>,
}

/// What happened this turn, as seen by Layer 4 after classify / stall observe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnOutcome {
    pub stop_reason: Option<StopReason>,
    pub stall_kind: Option<StallKind>,
    /// True when the tool passed pre-dispatch gates and was invoked (or served
    /// from cache after a successful evaluate). False for evaluate rejects.
    pub dispatched: bool,
    pub tool_name: String,
    pub target_path: Option<PathBuf>,
}

/// Why an "unsupported" recovery fired — capability policy vs evidence thrash.
/// Same top-level recipe name, distinct guidance (Part 3 naming check).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnsupportedCause {
    /// Tool / path outside current policy scope.
    PolicyDenied,
    /// Permitted tool, but returned (or was) already-known evidence.
    DuplicateEvidence,
}

/// Bounded recovery recipe (final-design-plan.md §7.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryAction {
    /// Failed patch → one exact reread of the target, then patch/finish only.
    FailedPatch { path: PathBuf },
    /// Invalid arguments / shape-invalid patch → schema error + bounded retry.
    InvalidArguments {
        tool: String,
        target: Option<PathBuf>,
    },
    /// Unsupported / out-of-scope capability, or duplicate-evidence thrash.
    UnsupportedCapability {
        cause: UnsupportedCause,
        tool: String,
        target: Option<PathBuf>,
    },
    /// Missing evidence for a *relevant* unseen path — guidance only (no grant).
    MissingEvidence { path: PathBuf },
    /// Repeated verification → require patch/finish next (cached block already applied).
    RepeatedVerification,
}

/// Why the run may complete without a hard failure (Slice 5/6 will own these).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionReason {
    TaskComplete,
    CompletedUnverified,
    BlockedWithoutEdit,
    NoProgress,
}

/// The only Layer 4 outputs. Callers must not invent a fifth branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeTransition {
    Continue,
    Recover(RecoveryAction),
    Complete(CompletionReason),
    Terminate(StopReason),
}

/// Decide the next runtime transition from a classified turn outcome.
///
/// Pure: does not mutate `state`. Prefer `Terminate` when a stop is already
/// stamped; otherwise select a §7.2 recipe from stall kind + dispatch bit.
pub fn decide_next_state(state: &AgentState, outcome: &TurnOutcome) -> RuntimeTransition {
    if let Some(reason) = &outcome.stop_reason {
        return RuntimeTransition::Terminate(reason.clone());
    }

    let Some(stall) = outcome.stall_kind else {
        return RuntimeTransition::Continue;
    };

    match stall {
        StallKind::FailedMutation => {
            if outcome.tool_name == "apply_patch" {
                if outcome.dispatched {
                    if let Some(path) = outcome.target_path.clone() {
                        return RuntimeTransition::Recover(RecoveryAction::FailedPatch { path });
                    }
                }
                // Finding 4: shape-invalid / pre-dispatch patch failure.
                return RuntimeTransition::Recover(RecoveryAction::InvalidArguments {
                    tool: outcome.tool_name.clone(),
                    target: outcome.target_path.clone(),
                });
            }
            RuntimeTransition::Recover(RecoveryAction::InvalidArguments {
                tool: outcome.tool_name.clone(),
                target: outcome.target_path.clone(),
            })
        }
        StallKind::InvalidArguments => {
            RuntimeTransition::Recover(RecoveryAction::InvalidArguments {
                tool: outcome.tool_name.clone(),
                target: outcome.target_path.clone(),
            })
        }
        StallKind::ScopeViolation => {
            if outcome.tool_name == "read_file" {
                if let Some(path) = outcome.target_path.as_ref() {
                    if !state.run_control.has_seen_file(path)
                        && is_relevant_missing_evidence(state, path)
                    {
                        return RuntimeTransition::Recover(RecoveryAction::MissingEvidence {
                            path: path.clone(),
                        });
                    }
                }
            }
            RuntimeTransition::Recover(RecoveryAction::UnsupportedCapability {
                cause: UnsupportedCause::PolicyDenied,
                tool: outcome.tool_name.clone(),
                target: outcome.target_path.clone(),
            })
        }
        StallKind::DuplicateEvidence => {
            RuntimeTransition::Recover(RecoveryAction::UnsupportedCapability {
                cause: UnsupportedCause::DuplicateEvidence,
                tool: outcome.tool_name.clone(),
                target: outcome.target_path.clone(),
            })
        }
        StallKind::RepeatedVerification => {
            RuntimeTransition::Recover(RecoveryAction::RepeatedVerification)
        }
        StallKind::NonResolvingResponse => RuntimeTransition::Continue,
    }
}

/// Apply a §7.2 recovery recipe: mutate Layer 1 / append guidance to `result`.
///
/// `stop_reason` is set when the recipe itself terminates the run (FailedPatch
/// budget, or Part 4 general-recipe fingerprint escalation).
pub fn apply_recovery(
    state: &mut AgentState,
    result: &mut ToolResult,
    action: &RecoveryAction,
) -> RecoveryApplyResult {
    match action {
        RecoveryAction::FailedPatch { path } => {
            // Target-specific budget owns terminate — not the general stall
            // counter, not guidance_attempts.
            apply_failed_patch(state, result, path)
        }
        other => apply_general_recipe(state, result, other),
    }
}

fn apply_general_recipe(
    state: &mut AgentState,
    result: &mut ToolResult,
    action: &RecoveryAction,
) -> RecoveryApplyResult {
    let key = recovery_key(action);
    let mut attempt = state.recovery.note_guidance_attempt(key);

    // Part 4: identical/coarse-repeat stalls escalate at least to tier 2.
    let exact_repeat = state.stall.is_repeat();
    let coarse_repeat = state.stall.coarse_is_repeat();
    let ping_pong = state.stall.is_ping_pong();
    if exact_repeat || coarse_repeat {
        attempt = attempt.max(2);
    }

    let guidance = match action {
        RecoveryAction::InvalidArguments { tool, .. } => {
            invalid_arguments_guidance(tool, attempt)
        }
        RecoveryAction::UnsupportedCapability {
            cause: UnsupportedCause::PolicyDenied,
            ..
        } => unsupported_capability_guidance(state, attempt),
        RecoveryAction::UnsupportedCapability {
            cause: UnsupportedCause::DuplicateEvidence,
            ..
        } => duplicate_evidence_guidance(state, attempt),
        RecoveryAction::MissingEvidence { path } => {
            missing_evidence_guidance(path, attempt)
        }
        RecoveryAction::RepeatedVerification => repeated_verification_guidance(attempt),
        RecoveryAction::FailedPatch { .. } => unreachable!("handled above"),
    };
    append_guidance(result, &guidance);

    // Part 4: terminate general recipes on same-key exhaustion with a
    // fingerprint stuckness signal. Ping-pong across keys is also caught
    // when consecutive stalls are already high.
    let fingerprint_stuck = exact_repeat || coarse_repeat || ping_pong;
    let key_exhausted = state.recovery.guidance_attempts() >= GENERAL_RECIPE_TERMINATE_ATTEMPTS;
    let ping_pong_exhausted = ping_pong && state.stall.consecutive_stalls >= 4;
    if (key_exhausted && fingerprint_stuck) || ping_pong_exhausted {
        let stop_msg = format!(
            "Stopped: recovery recipe exhausted after repeated non-progress \
             (exact_repeat={exact_repeat}, coarse_repeat={coarse_repeat}, ping_pong={ping_pong}). \
             apply_patch a seen file, update_plan, or finish with blockers."
        );
        append_guidance(result, &stop_msg);
        result.stop_reason = Some(StopReason::NoProgress);
        return RecoveryApplyResult {
            stop_reason: Some(StopReason::NoProgress),
            phase_guidance: None,
        };
    }

    RecoveryApplyResult {
        stop_reason: None,
        phase_guidance: None,
    }
}

fn recovery_key(action: &RecoveryAction) -> RecoveryKey {
    match action {
        RecoveryAction::FailedPatch { path } => {
            RecoveryKey::new("failed_patch", "apply_patch", Some(path.clone()))
        }
        RecoveryAction::InvalidArguments { tool, target } => {
            RecoveryKey::new("invalid_arguments", tool.clone(), target.clone())
        }
        RecoveryAction::UnsupportedCapability {
            cause,
            tool,
            target,
        } => {
            let reason = match cause {
                UnsupportedCause::PolicyDenied => "unsupported_capability",
                UnsupportedCause::DuplicateEvidence => "duplicate_evidence",
            };
            RecoveryKey::new(reason, tool.clone(), target.clone())
        }
        RecoveryAction::MissingEvidence { path } => {
            RecoveryKey::new("missing_evidence", "read_file", Some(path.clone()))
        }
        RecoveryAction::RepeatedVerification => {
            RecoveryKey::new("repeated_verification", "verify", None)
        }
    }
}

/// Unseen `read_file` is MissingEvidence only with a bounded relevance signal.
fn is_relevant_missing_evidence(state: &AgentState, path: &Path) -> bool {
    // Active / granted failed-patch target.
    if state
        .recovery
        .target()
        .is_some_and(|t| path_eq(t, path))
    {
        return true;
    }
    // Explicitly modified this run.
    if state
        .task
        .modified_files
        .iter()
        .any(|p| path_eq(p, path))
    {
        return true;
    }
    // Named in the goal.
    if path_mentioned_in_text(&state.goal, path) {
        return true;
    }
    // Named in the current plan.
    if let Some(plan) = state.task.plan.as_ref() {
        if path_mentioned_in_plan(plan, path) {
            return true;
        }
    }
    // Named in recent decisions / objective memory.
    if path_mentioned_in_task(&state.task, path) {
        return true;
    }
    false
}

fn path_mentioned_in_task(task: &TaskSessionState, path: &Path) -> bool {
    if let Some(obj) = task.objective.as_deref() {
        if path_mentioned_in_text(obj, path) {
            return true;
        }
    }
    task.decisions
        .iter()
        .any(|d| path_mentioned_in_text(&d.text, path))
}

fn path_mentioned_in_plan(plan: &AgentPlan, path: &Path) -> bool {
    if path_mentioned_in_text(&plan.objective, path) {
        return true;
    }
    if plan
        .steps
        .iter()
        .any(|s| path_mentioned_in_text(&s.text, path))
    {
        return true;
    }
    plan.constraints
        .iter()
        .any(|c| path_mentioned_in_text(&c.text, path))
}

fn path_mentioned_in_text(text: &str, path: &Path) -> bool {
    let display = path.to_string_lossy().replace('\\', "/");
    if !display.is_empty() && text.contains(display.as_str()) {
        return true;
    }
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        // Avoid matching tiny tokens like "a" / "ts".
        if name.len() >= 5 && text.contains(name) {
            return true;
        }
    }
    false
}

fn path_eq(a: &Path, b: &Path) -> bool {
    let na = a
        .to_string_lossy()
        .replace('\\', "/")
        .trim_start_matches("./")
        .to_string();
    let nb = b
        .to_string_lossy()
        .replace('\\', "/")
        .trim_start_matches("./")
        .to_string();
    na == nb
}

fn apply_failed_patch(
    state: &mut AgentState,
    result: &mut ToolResult,
    path: &Path,
) -> RecoveryApplyResult {
    let outcome = state.recovery.after_patch_failure(path, result.clone());
    *result = outcome.result;
    write_recovery_event(state, path);
    let phase_guidance = if outcome.show_phase_guidance {
        state.run_control.engage_patch_recovery_phase()
    } else {
        None
    };
    RecoveryApplyResult {
        stop_reason: result.stop_reason.clone(),
        phase_guidance,
    }
}

fn write_recovery_event(state: &AgentState, path: &Path) {
    crate::logging::write_event(
        &state.run_id,
        "patch_recovery_granted",
        &format!("{} (1 reread)", path.display()),
        0,
    );
}

fn append_guidance(result: &mut ToolResult, guidance: &str) {
    match &mut result.error {
        Some(e) => {
            e.push_str("\n\n");
            e.push_str(guidance);
        }
        None => result.error = Some(guidance.to_string()),
    }
}

fn invalid_arguments_guidance(tool: &str, attempt: u32) -> String {
    if attempt <= 1 {
        format!(
            "RECOVERY (invalid_arguments, attempt {attempt}): Fix the `{tool}` arguments and \
             you may retry this same tool once. Do not repeat the identical malformed call."
        )
    } else {
        format!(
            "RECOVERY (invalid_arguments, attempt {attempt}): Same-tool retry exhausted for `{tool}`. \
             Next action MUST be different — prefer apply_patch, update_plan, or finish with a \
             concrete code blocker. Repeating the same malformed call will not progress."
        )
    }
}

fn unsupported_capability_guidance(state: &AgentState, attempt: u32) -> String {
    let mut evidence = String::new();
    if !state.run_control.seen_files.is_empty() {
        evidence.push_str("\nFiles already in context:");
        let mut paths: Vec<_> = state.run_control.seen_files.iter().collect();
        paths.sort_by_key(|p| p.to_string_lossy().to_string());
        for path in paths.iter().take(8) {
            evidence.push_str(&format!("\n- {}", path.display()));
        }
    }
    if attempt <= 1 {
        format!(
            "RECOVERY (unsupported_capability, attempt {attempt}): That action is not available \
             under the current policy. Permitted next steps: apply_patch on a seen file, \
             update_plan, or finish with a concrete blocker.{evidence}"
        )
    } else {
        format!(
            "RECOVERY (unsupported_capability, attempt {attempt}): Still requesting an unavailable \
             action. Further exploration/search/unscoped reads will not be granted. \
             Next response MUST call apply_patch on a seen file, update_plan, or finish.{evidence}"
        )
    }
}

fn duplicate_evidence_guidance(state: &AgentState, attempt: u32) -> String {
    let mut evidence = String::new();
    if !state.run_control.seen_files.is_empty() {
        evidence.push_str("\nCached / already-seen files:");
        let mut paths: Vec<_> = state.run_control.seen_files.iter().collect();
        paths.sort_by_key(|p| p.to_string_lossy().to_string());
        for path in paths.iter().take(8) {
            evidence.push_str(&format!("\n- {}", path.display()));
        }
    }
    if attempt <= 1 {
        format!(
            "RECOVERY (duplicate_evidence, attempt {attempt}): That read/search returned evidence \
             already in context (or was denied as a duplicate). Do not re-fetch the same content. \
             Next action MUST be a different family — apply_patch, update_plan, or finish.{evidence}"
        )
    } else {
        format!(
            "RECOVERY (duplicate_evidence, attempt {attempt}): Still thrashing on known evidence. \
             Re-reads/searches of cached content will not progress. apply_patch a seen file, \
             update_plan, or finish summarizing what you already know.{evidence}"
        )
    }
}

fn missing_evidence_guidance(path: &Path, attempt: u32) -> String {
    let target = path.display();
    if attempt <= 1 {
        format!(
            "RECOVERY (missing_evidence, attempt {attempt}): `{target}` is relevant but not in \
             the seen-file scope for this phase. Name it in update_plan as a dependency, finish \
             with that blocker, or patch a file already in context — unrestricted exploration \
             will not reopen."
        )
    } else {
        format!(
            "RECOVERY (missing_evidence, attempt {attempt}): Repeated request for unseen `{target}` \
             denied. Next action MUST be apply_patch (seen file), update_plan naming `{target}`, \
             or finish with that blocker."
        )
    }
}

fn repeated_verification_guidance(attempt: u32) -> String {
    if attempt <= 1 {
        format!(
            "RECOVERY (repeated_verification, attempt {attempt}): That check was already run \
             (or blocked as identical). Do not re-run the same verification. apply_patch a fix \
             or finish summarizing the current result."
        )
    } else {
        format!(
            "RECOVERY (repeated_verification, attempt {attempt}): Verification retry exhausted. \
             Next action MUST be apply_patch or finish — repeating the same check will not progress."
        )
    }
}

/// Collapse a transition to the `Option<StopReason>` the agent loop understands.
///
/// `Recover` must be applied via [`apply_recovery`] *before* calling this;
/// leftover `Recover` here is a programming error.
pub fn stop_reason_from_transition(transition: RuntimeTransition) -> Option<StopReason> {
    match transition {
        RuntimeTransition::Continue => None,
        RuntimeTransition::Terminate(reason) => Some(reason),
        RuntimeTransition::Recover(action) => {
            panic!(
                "Recover({action:?}) must be applied via apply_recovery before stop_reason_from_transition"
            )
        }
        RuntimeTransition::Complete(reason) => {
            panic!(
                "decide_next_state returned Complete({reason:?}) before Slice 5/6 wired completion"
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::stall::StallState;
    use crate::state::step::Action;
    use crate::state::AgentPlan;

    fn empty_state() -> AgentState {
        AgentState::new("goal", PathBuf::from("/tmp"), "test-model", 10)
    }

    fn outcome(
        stall: Option<StallKind>,
        dispatched: bool,
        tool: &str,
        path: Option<&str>,
    ) -> TurnOutcome {
        TurnOutcome {
            stop_reason: None,
            stall_kind: stall,
            dispatched,
            tool_name: tool.into(),
            target_path: path.map(PathBuf::from),
        }
    }

    #[test]
    fn preexisting_stop_terminates() {
        let state = empty_state();
        let t = decide_next_state(
            &state,
            &TurnOutcome {
                stop_reason: Some(StopReason::NoProgress),
                stall_kind: Some(StallKind::FailedMutation),
                dispatched: true,
                tool_name: "apply_patch".into(),
                target_path: Some(PathBuf::from("a.rs")),
            },
        );
        assert_eq!(t, RuntimeTransition::Terminate(StopReason::NoProgress));
    }

    #[test]
    fn dispatched_patch_failure_selects_failed_patch_recipe() {
        let state = empty_state();
        let t = decide_next_state(
            &state,
            &outcome(
                Some(StallKind::FailedMutation),
                true,
                "apply_patch",
                Some("a.rs"),
            ),
        );
        assert_eq!(
            t,
            RuntimeTransition::Recover(RecoveryAction::FailedPatch {
                path: PathBuf::from("a.rs"),
            })
        );
    }

    #[test]
    fn shape_invalid_patch_selects_invalid_arguments_not_failed_patch() {
        let state = empty_state();
        let t = decide_next_state(
            &state,
            &outcome(
                Some(StallKind::FailedMutation),
                false,
                "apply_patch",
                Some("a.rs"),
            ),
        );
        assert_eq!(
            t,
            RuntimeTransition::Recover(RecoveryAction::InvalidArguments {
                tool: "apply_patch".into(),
                target: Some(PathBuf::from("a.rs")),
            })
        );
    }

    #[test]
    fn invalid_arguments_second_guidance_differs_from_first() {
        let mut state = empty_state();
        state.stall = StallState::new();
        let action = Action::ApplyPatch {
            path: PathBuf::from("a.rs"),
            diff: "bad".into(),
        };
        state
            .stall
            .observe(&action, Some(StallKind::InvalidArguments));
        let mut r1 = ToolResult::error("apply_patch.diff must be a unified diff");
        let _ = apply_recovery(
            &mut state,
            &mut r1,
            &RecoveryAction::InvalidArguments {
                tool: "apply_patch".into(),
                target: Some(PathBuf::from("a.rs")),
            },
        );
        let g1 = r1.error.clone().unwrap();

        state
            .stall
            .observe(&action, Some(StallKind::InvalidArguments));
        let mut r2 = ToolResult::error("apply_patch.diff must be a unified diff");
        let _ = apply_recovery(
            &mut state,
            &mut r2,
            &RecoveryAction::InvalidArguments {
                tool: "apply_patch".into(),
                target: Some(PathBuf::from("a.rs")),
            },
        );
        let g2 = r2.error.clone().unwrap();

        assert_ne!(g1, g2);
        assert!(g1.contains("attempt 1"));
        assert!(g2.contains("attempt 2"));
        assert!(g2.contains("Same-tool retry exhausted"));
        assert_eq!(state.recovery.guidance_attempts(), 2);
    }

    #[test]
    fn alternating_recipes_do_not_share_attempt_sequence() {
        let mut state = empty_state();
        let mut r = ToolResult::error("shape");
        let _ = apply_recovery(
            &mut state,
            &mut r,
            &RecoveryAction::InvalidArguments {
                tool: "apply_patch".into(),
                target: Some(PathBuf::from("a.rs")),
            },
        );
        assert_eq!(state.recovery.guidance_attempts(), 1);
        assert!(r.error.as_deref().unwrap().contains("attempt 1"));

        let mut r2 = ToolResult::error("unseen");
        let _ = apply_recovery(
            &mut state,
            &mut r2,
            &RecoveryAction::MissingEvidence {
                path: PathBuf::from("relevant_helper.ts"),
            },
        );
        assert_eq!(state.recovery.guidance_attempts(), 1);
        assert!(r2.error.as_deref().unwrap().contains("attempt 1"));
        assert!(r2.error.as_deref().unwrap().contains("missing_evidence"));
    }

    #[test]
    fn failed_patch_apply_grants_reread_and_third_stops() {
        let mut state = empty_state();
        let path = PathBuf::from("a.rs");
        for i in 1..=3 {
            state.stall.observe(
                &Action::ApplyPatch {
                    path: path.clone(),
                    diff: "x".into(),
                },
                Some(StallKind::FailedMutation),
            );
            let mut result = ToolResult::error(format!("hunk mismatch {i}"));
            let applied = apply_recovery(
                &mut state,
                &mut result,
                &RecoveryAction::FailedPatch {
                    path: path.clone(),
                },
            );
            if i < 3 {
                assert!(applied.stop_reason.is_none());
                assert!(applied.phase_guidance.is_some() || i > 1);
                assert!(state.recovery.is_active());
                assert!(state.recovery.has_grant_for(&path));
            } else {
                assert_eq!(applied.stop_reason, Some(StopReason::NoProgress));
            }
        }
        // FailedPatch budget must not be confused with guidance_attempts.
        assert_eq!(state.recovery.guidance_attempts(), 0);
        assert_eq!(state.recovery.consecutive_failures(), 3);
    }

    #[test]
    fn random_unseen_read_is_unsupported_not_missing_evidence() {
        let state = empty_state();
        let t = decide_next_state(
            &state,
            &outcome(
                Some(StallKind::ScopeViolation),
                false,
                "read_file",
                Some("random_unrelated.rs"),
            ),
        );
        assert_eq!(
            t,
            RuntimeTransition::Recover(RecoveryAction::UnsupportedCapability {
                cause: UnsupportedCause::PolicyDenied,
                tool: "read_file".into(),
                target: Some(PathBuf::from("random_unrelated.rs")),
            })
        );
    }

    #[test]
    fn recovery_target_unseen_read_is_missing_evidence() {
        let mut state = empty_state();
        let path = PathBuf::from("task_status.ts");
        let _ = state
            .recovery
            .after_patch_failure(&path, ToolResult::error("hunk"));
        // Grant path is the recovery target; file is still unseen.
        let t = decide_next_state(
            &state,
            &outcome(
                Some(StallKind::ScopeViolation),
                false,
                "read_file",
                Some("task_status.ts"),
            ),
        );
        assert_eq!(
            t,
            RuntimeTransition::Recover(RecoveryAction::MissingEvidence {
                path: PathBuf::from("task_status.ts"),
            })
        );
    }

    #[test]
    fn plan_named_unseen_read_is_missing_evidence() {
        let mut state = empty_state();
        state.task.plan = Some(
            AgentPlan::from_update(
                "fix date rules",
                "- edit frontend-v2/lib/task-status.ts\n- verify board",
            )
            .unwrap(),
        );
        let t = decide_next_state(
            &state,
            &outcome(
                Some(StallKind::ScopeViolation),
                false,
                "read_file",
                Some("frontend-v2/lib/task-status.ts"),
            ),
        );
        assert!(matches!(
            t,
            RuntimeTransition::Recover(RecoveryAction::MissingEvidence { .. })
        ));
    }

    #[test]
    fn duplicate_evidence_guidance_differs_from_policy_denied() {
        let mut state = empty_state();
        state
            .run_control
            .note_file_seen(PathBuf::from("seen.rs"));
        let mut dup = ToolResult::error("duplicate read denied");
        let _ = apply_recovery(
            &mut state,
            &mut dup,
            &RecoveryAction::UnsupportedCapability {
                cause: UnsupportedCause::DuplicateEvidence,
                tool: "read_file".into(),
                target: Some(PathBuf::from("seen.rs")),
            },
        );
        let mut pol = ToolResult::error("out of scope");
        // Different key so attempt stays 1 for fair text compare.
        let _ = apply_recovery(
            &mut state,
            &mut pol,
            &RecoveryAction::UnsupportedCapability {
                cause: UnsupportedCause::PolicyDenied,
                tool: "search_code".into(),
                target: None,
            },
        );
        let d = dup.error.unwrap();
        let p = pol.error.unwrap();
        assert!(d.contains("duplicate_evidence"));
        assert!(d.contains("Cached / already-seen") || d.contains("already in context"));
        assert!(p.contains("unsupported_capability"));
        assert!(p.contains("not available"));
        assert_ne!(d, p);
    }

    #[test]
    fn progress_continues() {
        let state = empty_state();
        let t = decide_next_state(&state, &outcome(None, true, "read_file", Some("a.rs")));
        assert_eq!(t, RuntimeTransition::Continue);
    }

    #[test]
    fn part4_exact_repeat_escalates_and_can_terminate_general_recipe() {
        let mut state = empty_state();
        let action = Action::ApplyPatch {
            path: PathBuf::from("a.rs"),
            diff: "bad".into(),
        };
        let recipe = RecoveryAction::InvalidArguments {
            tool: "apply_patch".into(),
            target: Some(PathBuf::from("a.rs")),
        };
        for i in 1..=3 {
            state
                .stall
                .observe(&action, Some(StallKind::InvalidArguments));
            let mut result = ToolResult::error("malformed");
            let applied = apply_recovery(&mut state, &mut result, &recipe);
            if i < 3 {
                assert!(applied.stop_reason.is_none(), "i={i}");
            } else {
                // attempt 3 + exact_repeat → Part 4 terminate
                assert_eq!(applied.stop_reason, Some(StopReason::NoProgress));
                assert!(result
                    .error
                    .as_deref()
                    .unwrap()
                    .contains("recovery recipe exhausted"));
            }
        }
    }
}
