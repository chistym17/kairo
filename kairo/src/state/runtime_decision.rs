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

use crate::state::agent_state::AgentState;
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

/// Why an unseen path was treated as relevant for MissingEvidence.
/// Logged on every grant so corpus runs can audit basename false-positives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelevanceRule {
    RecoveryTarget,
    ModifiedFile,
    GoalFullPath,
    GoalBasename,
    PlanFullPath,
    PlanBasename,
    DecisionFullPath,
    DecisionBasename,
}

impl RelevanceRule {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RecoveryTarget => "recovery_target",
            Self::ModifiedFile => "modified_file",
            Self::GoalFullPath => "goal_full_path",
            Self::GoalBasename => "goal_basename",
            Self::PlanFullPath => "plan_full_path",
            Self::PlanBasename => "plan_basename",
            Self::DecisionFullPath => "decision_full_path",
            Self::DecisionBasename => "decision_basename",
        }
    }
}

/// Why a model turn produced no tool call (Part 5D).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoToolCallCause {
    /// Ordinary text / final-answer style response with no tools.
    TextOnly,
    /// Provider hit max_tokens (`finish_reason = length`) before a tool call.
    MaxTokens,
}

impl NoToolCallCause {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TextOnly => "text_only",
            Self::MaxTokens => "max_tokens",
        }
    }

    pub fn from_response_text(text: &str) -> Self {
        if text.contains("truncated: max_tokens reached") {
            Self::MaxTokens
        } else {
            Self::TextOnly
        }
    }
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
    /// Relevant unseen path → one bounded read grant (not full patch-recovery mode).
    MissingEvidence {
        path: PathBuf,
        relevance: RelevanceRule,
    },
    /// Repeated verification → require patch/finish next (cached block already applied).
    RepeatedVerification,
    /// Text-only / truncated turn with no tool call (Part 5D).
    NonResolving { cause: NoToolCallCause },
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
                    if !state.run_control.has_seen_file(path) {
                        if let Some(relevance) = classify_relevance(state, path) {
                            return RuntimeTransition::Recover(RecoveryAction::MissingEvidence {
                                path: path.clone(),
                                relevance,
                            });
                        }
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
        // Empty searches / non-resolving tool successes stay Continue.
        // Text-only NoToolCall commits apply `RecoveryAction::NonResolving`
        // directly (Part 5D) rather than relying on this branch.
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
    let key_attempt = state.recovery.note_guidance_attempt(key.clone());
    let mut attempt = key_attempt;

    // Part 4: identical/coarse-repeat stalls escalate at least to tier 2.
    let exact_repeat = state.stall.is_repeat();
    let coarse_repeat = state.stall.coarse_is_repeat();
    let ping_pong = state.stall.is_ping_pong();
    if exact_repeat || coarse_repeat {
        attempt = attempt.max(2);
    }

    let guidance = match action {
        RecoveryAction::InvalidArguments { tool, .. } => {
            invalid_arguments_guidance(state, tool, attempt)
        }
        RecoveryAction::UnsupportedCapability {
            cause: UnsupportedCause::PolicyDenied,
            tool,
            ..
        } => unsupported_capability_guidance(state, tool, attempt),
        RecoveryAction::UnsupportedCapability {
            cause: UnsupportedCause::DuplicateEvidence,
            tool: _,
            target,
        } => {
            // Part 5B: escalate Act read scope for this path (not FailedPatch mode).
            let mut level = 0u32;
            if let Some(path) = target {
                level = state.recovery.note_act_duplicate(path);
                let snap = duplicate_scope_snapshot_message(path, level);
                state.run_control.queue_phase_guidance(&snap);
                write_duplicate_escalation_event(state, path, level);
            }
            duplicate_evidence_guidance(state, attempt, level, target.as_deref())
        }
        RecoveryAction::MissingEvidence { path, relevance } => {
            // First *key* attempt: one bounded workspace-relative grant.
            // Fingerprint escalation must not skip the grant.
            let mut granted = false;
            if key_attempt == 1 {
                if let Some(norm) =
                    normalize_workspace_rel_file(&state.workspace_root, path)
                {
                    granted = state.recovery.grant_evidence_read(&norm, key.clone());
                    log_missing_evidence_relevance(state, &norm, *relevance, granted);
                } else {
                    log_missing_evidence_relevance(state, path, *relevance, false);
                }
            }
            missing_evidence_guidance(path, attempt, *relevance, granted)
        }
        RecoveryAction::RepeatedVerification => repeated_verification_guidance(attempt),
        RecoveryAction::NonResolving { cause } => {
            state.run_control.require_tool_choice = true;
            non_resolving_guidance(state, *cause, attempt)
        }
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
        RecoveryAction::MissingEvidence { path, .. } => {
            RecoveryKey::new("missing_evidence", "read_file", Some(path.clone()))
        }
        RecoveryAction::RepeatedVerification => {
            RecoveryKey::new("repeated_verification", "verify", None)
        }
        RecoveryAction::NonResolving { cause } => RecoveryKey::new(
            format!("non_resolving_{}", cause.as_str()),
            "no_tool_call",
            None,
        ),
    }
}

/// Unseen `read_file` is MissingEvidence only with a bounded relevance signal.
/// Returns which rule matched (for logging / corpus audit).
fn classify_relevance(state: &AgentState, path: &Path) -> Option<RelevanceRule> {
    if state
        .recovery
        .target()
        .is_some_and(|t| path_eq(t, path))
    {
        return Some(RelevanceRule::RecoveryTarget);
    }
    if state
        .task
        .modified_files
        .iter()
        .any(|p| path_eq(p, path))
    {
        return Some(RelevanceRule::ModifiedFile);
    }
    if let Some(rule) = match_in_text(
        &state.goal,
        path,
        RelevanceRule::GoalFullPath,
        RelevanceRule::GoalBasename,
    ) {
        return Some(rule);
    }
    if let Some(plan) = state.task.plan.as_ref() {
        if let Some(rule) = match_in_text(
            &plan.objective,
            path,
            RelevanceRule::PlanFullPath,
            RelevanceRule::PlanBasename,
        ) {
            return Some(rule);
        }
        for s in &plan.steps {
            if let Some(rule) = match_in_text(
                &s.text,
                path,
                RelevanceRule::PlanFullPath,
                RelevanceRule::PlanBasename,
            ) {
                return Some(rule);
            }
        }
        for c in &plan.constraints {
            if let Some(rule) = match_in_text(
                &c.text,
                path,
                RelevanceRule::PlanFullPath,
                RelevanceRule::PlanBasename,
            ) {
                return Some(rule);
            }
        }
    }
    if let Some(obj) = state.task.objective.as_deref() {
        if let Some(rule) = match_in_text(
            obj,
            path,
            RelevanceRule::DecisionFullPath,
            RelevanceRule::DecisionBasename,
        ) {
            return Some(rule);
        }
    }
    for d in &state.task.decisions {
        if let Some(rule) = match_in_text(
            &d.text,
            path,
            RelevanceRule::DecisionFullPath,
            RelevanceRule::DecisionBasename,
        ) {
            return Some(rule);
        }
    }
    None
}

fn match_in_text(
    text: &str,
    path: &Path,
    full: RelevanceRule,
    basename: RelevanceRule,
) -> Option<RelevanceRule> {
    let display = path.to_string_lossy().replace('\\', "/");
    if !display.is_empty() && text.contains(display.as_str()) {
        return Some(full);
    }
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        // Basename ≥5 reduces `a`/`ts` noise; common names like `utils` can
        // still false-positive — corpus logs `relevance` to measure that.
        if name.len() >= 5 && text.contains(name) {
            return Some(basename);
        }
    }
    None
}

/// Normalize to a workspace-relative path that names an existing regular file.
/// Rejects absolute paths, `..` escapes, directories, and missing files.
fn normalize_workspace_rel_file(workspace: &Path, path: &Path) -> Option<PathBuf> {
    if path.is_absolute() {
        return None;
    }
    let display = path.to_string_lossy().replace('\\', "/");
    if display.split('/').any(|c| c == "..") {
        return None;
    }
    let trimmed = display.trim_start_matches("./");
    if trimmed.is_empty() {
        return None;
    }
    let full = workspace.join(trimmed);
    let meta = std::fs::symlink_metadata(&full).ok()?;
    if !meta.is_file() {
        return None;
    }
    // Refuse symlink escape: canonicalize parent and ensure under workspace.
    let canon_file = full.canonicalize().ok()?;
    let canon_root = workspace.canonicalize().ok()?;
    if !canon_file.starts_with(&canon_root) {
        return None;
    }
    Some(PathBuf::from(trimmed))
}

fn log_missing_evidence_relevance(
    state: &AgentState,
    path: &Path,
    relevance: RelevanceRule,
    granted: bool,
) {
    let payload = serde_json::json!({
        "path": path.display().to_string(),
        "relevance": relevance.as_str(),
        "granted": granted,
        "recovery_key": state.recovery.guidance_key().map(|k| {
            serde_json::json!({
                "reason": k.reason,
                "tool": k.tool,
                "target": k.target.as_ref().map(|p| p.display().to_string()),
            })
        }),
    });
    crate::logging::write_event(
        &state.run_id,
        "missing_evidence_relevance",
        &payload.to_string(),
        0,
    );
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

fn allowed_next_block(state: &AgentState) -> String {
    crate::state::action_policy::allowed_next_actions(&state.run_control, &state.recovery)
        .render_for_guidance()
}

fn invalid_arguments_guidance(state: &AgentState, tool: &str, attempt: u32) -> String {
    let allowed = allowed_next_block(state);
    if attempt <= 1 {
        format!(
            "RECOVERY (invalid_arguments, attempt {attempt}): rejected `{tool}` — fix its \
             arguments and you may retry this same tool once. Do not repeat the identical \
             malformed call.\n{allowed}"
        )
    } else {
        format!(
            "RECOVERY (invalid_arguments, attempt {attempt}): Same-tool retry exhausted for \
             `{tool}`. Next action MUST be different and chosen from allowed_next_actions — \
             do not force one specific tool; repeating the same malformed call will not \
             progress.\n{allowed}"
        )
    }
}

fn unsupported_capability_guidance(state: &AgentState, tool: &str, attempt: u32) -> String {
    let allowed = allowed_next_block(state);
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
            "RECOVERY (unsupported_capability, attempt {attempt}): rejected `{tool}` — that \
             action is not available under the current policy. Choose the next action from \
             allowed_next_actions (do not assume a single required tool).{evidence}\n{allowed}"
        )
    } else {
        format!(
            "RECOVERY (unsupported_capability, attempt {attempt}): Still requesting an \
             unavailable action (`{tool}`). Further exploration/search/unscoped reads will \
             not be granted. Next response MUST use one of allowed_next_actions.{evidence}\n\
             {allowed}"
        )
    }
}

fn duplicate_evidence_guidance(
    state: &AgentState,
    attempt: u32,
    level: u32,
    path: Option<&Path>,
) -> String {
    let mut evidence = String::new();
    if !state.run_control.seen_files.is_empty() {
        evidence.push_str("\nCached / already-seen files:");
        let mut paths: Vec<_> = state.run_control.seen_files.iter().collect();
        paths.sort_by_key(|p| p.to_string_lossy().to_string());
        for p in paths.iter().take(8) {
            evidence.push_str(&format!("\n- {}", p.display()));
        }
    }
    let target = path
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "<path>".into());
    if level <= 1 {
        format!(
            "RECOVERY (duplicate_evidence, attempt {attempt}, scope=restrict): \
             The requested content for `{target}` is already available in context. \
             Further duplicate reads of this file are unavailable. Use existing evidence to \
             update_plan, apply_patch, or finish. A genuinely unread range of another seen \
             file may still be read once.{evidence}"
        )
    } else {
        format!(
            "RECOVERY (duplicate_evidence, attempt {attempt}, scope=disable_path): \
             `read_file` for `{target}` is disabled after repeated duplicate evidence. \
             Next action MUST be apply_patch, update_plan, or finish — not another read of \
             that path.{evidence}"
        )
    }
}

fn duplicate_scope_snapshot_message(path: &Path, level: u32) -> String {
    if level <= 1 {
        format!(
            "The requested content is already available in context.\n\
             Further duplicate reads of `{}` are unavailable.\n\
             Use the existing evidence to update the plan, apply a patch, or finish.\n\
             read_file remains available for other seen files / unread ranges.",
            path.display()
        )
    } else {
        format!(
            "read_file for `{}` is now disabled (duplicate evidence exhausted).\n\
             Available actions: update_plan, apply_patch, finish.\n\
             A different relevant unseen file may still be requested only via MissingEvidence rules.",
            path.display()
        )
    }
}

fn write_duplicate_escalation_event(state: &AgentState, path: &Path, level: u32) {
    let payload = serde_json::json!({
        "path": path.display().to_string(),
        "level": level,
        "restricted": level >= 1,
        "path_disabled": level >= 2,
    });
    crate::logging::write_event(
        &state.run_id,
        "duplicate_evidence_escalation",
        &payload.to_string(),
        0,
    );
}

fn missing_evidence_guidance(
    path: &Path,
    attempt: u32,
    relevance: RelevanceRule,
    granted: bool,
) -> String {
    let target = path.display();
    let grant_note = if granted {
        format!(
            " One bounded read of `{target}` is granted for this recovery key \
             (relevance={}).",
            relevance.as_str()
        )
    } else if attempt == 1 {
        format!(
            " No read grant issued (path must be a regular workspace-relative file; \
             relevance={}).",
            relevance.as_str()
        )
    } else {
        String::new()
    };
    if attempt <= 1 {
        format!(
            "RECOVERY (missing_evidence, attempt {attempt}): `{target}` is relevant but was \
             outside seen-file scope.{grant_note} After the granted read (if any), prefer \
             apply_patch, update_plan, or finish — unrestricted exploration will not reopen."
        )
    } else {
        format!(
            "RECOVERY (missing_evidence, attempt {attempt}): Repeated request for `{target}` \
             (relevance={}). Grant not refreshed. Next action MUST be apply_patch (seen file), \
             update_plan, or finish with that blocker.",
            relevance.as_str()
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

fn non_resolving_guidance(state: &AgentState, cause: NoToolCallCause, attempt: u32) -> String {
    let allowed = allowed_next_block(state);
    let cause_line = match cause {
        NoToolCallCause::MaxTokens => {
            "Your previous response was truncated before a tool call (max_tokens). \
             Use a tool immediately and keep any explanation brief."
        }
        NoToolCallCause::TextOnly => {
            "Your last response did not perform an action and the task is still incomplete. \
             Do not continue with a long analysis."
        }
    };
    if attempt <= 1 {
        format!(
            "RECOVERY (non_resolving, cause={}, attempt {attempt}): {cause_line}\n{allowed}",
            cause.as_str()
        )
    } else {
        format!(
            "RECOVERY (non_resolving, cause={}, attempt {attempt}): Still no tool call. \
             Next response MUST be exactly one tool from allowed_next_actions — \
             no essay, no continued analysis.\n{allowed}",
            cause.as_str()
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
    use crate::state::stall::{ActionFingerprint, StallState};
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
        assert!(g1.contains("allowed_next_actions:"));
        assert!(g2.contains("allowed_next_actions:"));
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
                relevance: RelevanceRule::PlanBasename,
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
                relevance: RelevanceRule::RecoveryTarget,
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
        assert!(d.contains("scope=restrict") || d.contains("scope=disable_path"));
        assert!(d.contains("Cached / already-seen") || d.contains("already in context"));
        assert!(p.contains("unsupported_capability"));
        assert!(p.contains("not available") || p.contains("rejected"));
        assert!(p.contains("allowed_next_actions:"));
        assert_ne!(d, p);
        assert_eq!(
            state
                .recovery
                .duplicate_escalation_level(Path::new("seen.rs")),
            1
        );
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

    #[test]
    fn part5b_duplicate_escalation_restricts_then_disables_same_path() {
        let mut state = empty_state();
        state
            .run_control
            .note_file_seen(PathBuf::from("seen.rs"));
        let path = PathBuf::from("seen.rs");
        let recipe = RecoveryAction::UnsupportedCapability {
            cause: UnsupportedCause::DuplicateEvidence,
            tool: "read_file".into(),
            target: Some(path.clone()),
        };

        let mut r1 = ToolResult::error("duplicate read denied");
        let a1 = apply_recovery(&mut state, &mut r1, &recipe);
        assert!(a1.stop_reason.is_none());
        assert_eq!(state.recovery.duplicate_escalation_level(&path), 1);
        assert!(state.recovery.is_act_duplicate_restricted(&path));
        assert!(!state.recovery.is_act_duplicate_path_disabled(&path));
        let g1 = r1.error.clone().unwrap();
        assert!(g1.contains("scope=restrict"));

        let mut r2 = ToolResult::error("duplicate read denied");
        let a2 = apply_recovery(&mut state, &mut r2, &recipe);
        assert!(a2.stop_reason.is_none());
        assert_eq!(state.recovery.duplicate_escalation_level(&path), 2);
        assert!(state.recovery.is_act_duplicate_path_disabled(&path));
        let g2 = r2.error.clone().unwrap();
        assert!(g2.contains("scope=disable_path"));
        assert_ne!(g1, g2);

        // Act schema still exposes read_file.
        assert!(state
            .run_control
            .effective_tools(&state.recovery)
            .contains(&"read_file"));

        // FailedPatch budget untouched.
        assert_eq!(state.recovery.consecutive_failures(), 0);
        assert!(!state.recovery.is_active());
    }

    #[test]
    fn part5b_update_plan_clears_duplicate_without_clearing_failed_patch() {
        let mut state = empty_state();
        let patch_path = PathBuf::from("a.rs");
        let _ = state
            .recovery
            .after_patch_failure(&patch_path, ToolResult::error("hunk"));
        assert!(state.recovery.is_active());
        assert_eq!(state.recovery.consecutive_failures(), 1);

        let _ = state.recovery.note_act_duplicate(Path::new("seen.rs"));
        assert_eq!(
            state
                .recovery
                .duplicate_escalation_level(Path::new("seen.rs")),
            1
        );

        state.recovery.clear_duplicate_escalation();
        assert_eq!(
            state
                .recovery
                .duplicate_escalation_level(Path::new("seen.rs")),
            0
        );
        assert!(state.recovery.is_active());
        assert_eq!(state.recovery.consecutive_failures(), 1);
        assert!(state.recovery.has_grant_for(&patch_path));
    }

    #[test]
    fn part5b_successful_patch_clear_wipes_duplicate_escalation() {
        let mut state = empty_state();
        let _ = state.recovery.note_act_duplicate(Path::new("seen.rs"));
        assert!(state.recovery.is_act_duplicate_restricted(Path::new("seen.rs")));
        state.recovery.clear();
        assert_eq!(
            state
                .recovery
                .duplicate_escalation_level(Path::new("seen.rs")),
            0
        );
    }

    #[test]
    fn part5b_missing_evidence_still_possible_for_other_relevant_unseen() {
        let mut state = empty_state();
        state
            .run_control
            .note_file_seen(PathBuf::from("seen.rs"));
        let _ = state.recovery.note_act_duplicate(Path::new("seen.rs"));
        let _ = state.recovery.note_act_duplicate(Path::new("seen.rs"));
        assert!(state
            .recovery
            .is_act_duplicate_path_disabled(Path::new("seen.rs")));

        // Plan names a different unseen file → MissingEvidence, not blocked
        // by the seen.rs duplicate escalation.
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
    fn part2_policy_denied_locate_in_act_cites_allowed_next_snapshot() {
        let mut state = empty_state();
        assert!(state.run_control.enter_act());
        state
            .run_control
            .note_file_seen(PathBuf::from("Board.tsx"));
        let _ = state
            .recovery
            .note_act_duplicate(Path::new("Board.tsx"));
        let _ = state
            .recovery
            .note_act_duplicate(Path::new("Board.tsx"));

        let mut result = ToolResult::error("locate unavailable");
        let _ = apply_recovery(
            &mut state,
            &mut result,
            &RecoveryAction::UnsupportedCapability {
                cause: UnsupportedCause::PolicyDenied,
                tool: "locate".into(),
                target: None,
            },
        );
        let g = result.error.unwrap();
        assert!(g.contains("rejected `locate`"));
        assert!(g.contains("allowed_next_actions:"));
        assert!(g.contains("- read_file"));
        assert!(g.contains("- update_plan"));
        assert!(g.contains("- apply_patch"));
        assert!(g.contains("- finish"));
        assert!(!g.contains("- locate"));
        assert!(g.contains("constraints:"));
        assert!(g.contains("Board.tsx"));
        assert!(g.contains("path disabled"));
        // Must not force a single tool.
        assert!(!g.contains("MUST call apply_patch"));
    }

    #[test]
    fn part2_policy_denied_attempt_guidance_differs_and_keeps_snapshot() {
        let mut state = empty_state();
        assert!(state.run_control.enter_act());
        let recipe = RecoveryAction::UnsupportedCapability {
            cause: UnsupportedCause::PolicyDenied,
            tool: "locate".into(),
            target: None,
        };
        let mut r1 = ToolResult::error("locate unavailable");
        let _ = apply_recovery(&mut state, &mut r1, &recipe);
        let g1 = r1.error.unwrap();

        let mut r2 = ToolResult::error("locate unavailable");
        let _ = apply_recovery(&mut state, &mut r2, &recipe);
        let g2 = r2.error.unwrap();

        assert_ne!(g1, g2);
        assert!(g1.contains("attempt 1"));
        assert!(g2.contains("attempt 2"));
        assert!(g1.contains("allowed_next_actions:"));
        assert!(g2.contains("allowed_next_actions:"));
        assert!(g2.contains("Still requesting an unavailable action"));
    }

    #[test]
    fn part3_same_key_policy_denied_with_fingerprint_terminates_via_layer4() {
        let mut state = empty_state();
        assert!(state.run_control.enter_act());
        let action = Action::Locate {
            query: "board".into(),
            path: PathBuf::from("frontend-v2"),
        };
        let recipe = RecoveryAction::UnsupportedCapability {
            cause: UnsupportedCause::PolicyDenied,
            tool: "locate".into(),
            target: None,
        };
        for i in 1..=3 {
            state
                .stall
                .observe(&action, Some(StallKind::ScopeViolation));
            let mut result = ToolResult::error("locate unavailable");
            let applied = apply_recovery(&mut state, &mut result, &recipe);
            if i < 3 {
                assert!(applied.stop_reason.is_none(), "i={i}");
            } else {
                assert_eq!(applied.stop_reason, Some(StopReason::NoProgress));
                assert!(result
                    .error
                    .as_deref()
                    .unwrap()
                    .contains("recovery recipe exhausted"));
            }
        }
    }

    #[test]
    fn part5d_max_tokens_no_tool_recovery_cites_allowed_next_and_terminates() {
        let mut state = empty_state();
        assert!(state.run_control.enter_act());
        let recipe = RecoveryAction::NonResolving {
            cause: NoToolCallCause::MaxTokens,
        };
        let action_fp = ActionFingerprint::no_tool_call_cause("max_tokens");
        for i in 1..=3 {
            state.stall.observe_fingerprint(
                action_fp.clone(),
                Some(StallKind::NonResolvingResponse),
            );
            let mut result = ToolResult::error("no tool");
            let applied = apply_recovery(&mut state, &mut result, &recipe);
            let g = result.error.clone().unwrap();
            assert!(g.contains("allowed_next_actions:"), "{g}");
            assert!(g.contains("max_tokens") || g.contains("truncated"), "{g}");
            assert!(state.run_control.require_tool_choice);
            if i < 3 {
                assert!(applied.stop_reason.is_none(), "i={i}");
                if i == 1 {
                    assert!(g.contains("truncated before a tool call") || g.contains("max_tokens"));
                    assert!(!g.contains("Still no tool call"));
                } else {
                    assert!(g.contains("Still no tool call"));
                }
            } else {
                assert_eq!(applied.stop_reason, Some(StopReason::NoProgress));
            }
        }
    }

    #[test]
    fn part5d_different_essays_same_cause_still_escalate() {
        let mut state = empty_state();
        assert!(state.run_control.enter_act());
        let recipe = RecoveryAction::NonResolving {
            cause: NoToolCallCause::TextOnly,
        };
        // Stable cause fingerprint — essay bytes must not reset the key.
        let fp = ActionFingerprint::no_tool_call_cause("text_only");
        for i in 1..=3 {
            state
                .stall
                .observe_fingerprint(fp.clone(), Some(StallKind::NonResolvingResponse));
            let mut result = ToolResult::error(format!("essay variant {i}"));
            let applied = apply_recovery(&mut state, &mut result, &recipe);
            if i < 3 {
                assert!(applied.stop_reason.is_none(), "i={i}");
            } else {
                assert_eq!(applied.stop_reason, Some(StopReason::NoProgress));
            }
        }
    }

    #[test]
    fn no_tool_call_cause_detects_max_tokens_marker() {
        assert_eq!(
            NoToolCallCause::from_response_text(
                "[truncated: max_tokens reached with no tool call] analysis..."
            ),
            NoToolCallCause::MaxTokens
        );
        assert_eq!(
            NoToolCallCause::from_response_text("here is my analysis of the bug"),
            NoToolCallCause::TextOnly
        );
    }
}
