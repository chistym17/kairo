//! Generic invalid-action recovery for phase contracts.
//!
//! Schema filtering is the first defense; this validator is the second when the
//! model (or provider) still emits tools/args that violate the current phase.

use std::collections::HashSet;

use crate::state::phase::{Phase, RunState, ACT_TOOLS, FINISH_TOOLS, PATCH_RECOVERY_TOOLS, VERIFY_TOOLS};
use crate::state::recovery::RecoveryState;
use crate::state::step::Action;
use crate::state::stop_reason::StopReason;
use crate::state::tool_scope::{ScopeBudget, ToolScope};
use crate::tools::ToolResult;

/// Why a requested action is invalid for the current runtime state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidActionReason {
    ToolUnavailable,
    ArgumentPolicyViolation,
    RecoveryTargetMismatch,
    PhaseAlreadyComplete,
}

impl InvalidActionReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ToolUnavailable => "tool_unavailable",
            Self::ArgumentPolicyViolation => "argument_policy_violation",
            Self::RecoveryTargetMismatch => "recovery_target_mismatch",
            Self::PhaseAlreadyComplete => "phase_already_complete",
        }
    }
}

/// Terminal resolution after repeated invalid actions (phase-specific).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidFallback {
    /// Move Explore → Act and continue with a correction.
    TransitionToAct,
    /// Stop as no-progress / finish with blocker.
    FinishWithBlocker,
    /// Stop as completed unverified.
    FinishUnverified,
    /// Stop as task complete (or unverified if never green).
    AutoComplete,
    /// Hard stop.
    StopNoProgress,
}

/// Contract for the current phase / recovery mode.
#[derive(Debug, Clone, Copy)]
pub struct PhaseContract {
    pub allowed_tools: &'static [&'static str],
    pub max_invalid_actions: usize,
    pub fallback: InvalidFallback,
    pub phase_label: &'static str,
}

/// Outcome of [`validate_action`].
#[derive(Debug, Clone)]
pub enum ActionDecision {
    Allow,
    RejectAndRetry {
        reason: InvalidActionReason,
        correction: String,
        /// Ask the next LLM request to use `tool_choice=required` when supported.
        require_tool_choice: bool,
    },
    /// Enter Act (from Explore) then reject with a correction for the next turn.
    TransitionToAct {
        correction: String,
    },
    Stop {
        reason: StopReason,
        message: String,
    },
}

/// Default max invalid actions before phase fallback (Explore / Act).
pub const DEFAULT_MAX_INVALID_ACTIONS: usize = 3;

pub fn contract_for(run: &RunState, recovery: &RecoveryState) -> PhaseContract {
    if recovery.is_active() {
        return PhaseContract {
            allowed_tools: PATCH_RECOVERY_TOOLS,
            max_invalid_actions: DEFAULT_MAX_INVALID_ACTIONS,
            fallback: InvalidFallback::StopNoProgress,
            phase_label: "patch_recovery",
        };
    }
    match run.phase {
        Phase::Explore => PhaseContract {
            allowed_tools: run.phase.allowed_tools(),
            max_invalid_actions: DEFAULT_MAX_INVALID_ACTIONS,
            fallback: InvalidFallback::TransitionToAct,
            phase_label: "explore",
        },
        Phase::Act => PhaseContract {
            // read_file is present (Slice 1) but bounded to seen_files — see
            // classify_invalid's act_read_scope, not this list, for that gate.
            allowed_tools: ACT_TOOLS,
            max_invalid_actions: DEFAULT_MAX_INVALID_ACTIONS,
            fallback: InvalidFallback::FinishWithBlocker,
            phase_label: "act",
        },
        Phase::Verify => PhaseContract {
            allowed_tools: VERIFY_TOOLS,
            max_invalid_actions: 2,
            fallback: InvalidFallback::FinishUnverified,
            phase_label: "verify",
        },
        Phase::Finish => PhaseContract {
            allowed_tools: FINISH_TOOLS,
            max_invalid_actions: 1,
            fallback: InvalidFallback::AutoComplete,
            phase_label: "finish",
        },
    }
}

/// Validate a model tool call against the current phase contract.
///
/// Updates `consecutive_invalid_actions` on `run`. Callers apply the decision
/// (commit deny, queue guidance, stop, transition).
pub fn validate_action(
    run: &mut RunState,
    recovery: &RecoveryState,
    action: &Action,
) -> ActionDecision {
    let contract = contract_for(run, recovery);
    let name = action_tool_name(action);

    let invalid = classify_invalid(run, recovery, action, name, &contract);

    match invalid {
        None => {
            run.note_valid_action();
            ActionDecision::Allow
        }
        Some(reason) => escalate_invalid(run, name, reason, &contract),
    }
}

fn classify_invalid(
    run: &RunState,
    recovery: &RecoveryState,
    action: &Action,
    name: &str,
    contract: &PhaseContract,
) -> Option<InvalidActionReason> {
    if matches!(run.phase, Phase::Finish) && name != "finish" {
        return Some(InvalidActionReason::PhaseAlreadyComplete);
    }

    if !contract.allowed_tools.iter().any(|t| *t == name) {
        return Some(InvalidActionReason::ToolUnavailable);
    }

    // Tool allowed — check argument policies.
    if let Action::Finish { summary } = action {
        if run.act_force_edit
            && matches!(run.phase, Phase::Act)
            && !recovery.is_active()
            && is_escape_finish(summary)
        {
            return Some(InvalidActionReason::ArgumentPolicyViolation);
        }
    }

    if let Action::ReadFile { path, .. } = action {
        if recovery.is_active() {
            if !patch_recovery_scope(recovery).permits_path(path) {
                return Some(InvalidActionReason::RecoveryTargetMismatch);
            }
        } else if matches!(run.phase, Phase::Act) {
            // Slice 1 (final-design-plan.md §5.1, §8): read_file stays visible
            // in Act's schema, but a read outside the seen-files scope is
            // still rejected — as a uniform ArgumentPolicyViolation, not by
            // the tool vanishing from the schema.
            if !act_read_scope(run).permits_path(path) {
                return Some(InvalidActionReason::ArgumentPolicyViolation);
            }
        } else if matches!(run.phase, Phase::Verify) {
            // Verify's read gate predates ToolScope and is left as-is for
            // this slice — Slice 1's hypothesis is scoped to Act only
            // (final-design-plan.md §8, Slice 1 "Scope").
            if !run.has_seen_file(path) && run.justified_unseen_reads_left == 0 {
                return Some(InvalidActionReason::ArgumentPolicyViolation);
            }
        }
    }

    None
}

/// The `ToolScope` governing `read_file` in Act: bounded to files already
/// seen this run, unlimited re-reads of those. Recomputed fresh from
/// `RunState` on every check (final-design-plan.md §5.2) rather than stored.
///
/// An empty `seen_files` set cannot go through `ToolScope::bounded_paths`
/// (its non-empty-set invariant would panic — see `tool_scope.rs`), so it is
/// treated as `disabled()`: with no evidence yet, no read is in scope, same
/// outcome as before this scope existed.
fn act_read_scope(run: &RunState) -> ToolScope {
    if run.seen_files.is_empty() {
        ToolScope::disabled()
    } else {
        ToolScope::bounded_paths(run.seen_files.clone(), ScopeBudget::Unlimited)
    }
}

/// The `ToolScope` governing `read_file` during patch recovery: bounded to
/// the single failed-patch target (final-design-plan.md §5.2's recovery
/// example). `RecoveryState::target` is always `Some` whenever recovery is
/// active — the `disabled()` fallback exists only so this function cannot
/// panic if that invariant is ever violated.
///
/// Budget is deliberately left `Unlimited` here rather than `Uses(1)`: the
/// real one-shot consumption for this grant already happens correctly at
/// dispatch time via `RecoveryState::try_consume_read` (`loop_engine.rs`).
/// Reporting `Uses(1)` here regardless of whether the grant was already
/// spent this turn would make the `ToolScope` lie about its own budget.
fn patch_recovery_scope(recovery: &RecoveryState) -> ToolScope {
    match recovery.target() {
        Some(path) => {
            let paths: HashSet<_> = std::iter::once(path.to_path_buf()).collect();
            ToolScope::bounded_paths(paths, ScopeBudget::Unlimited)
        }
        None => ToolScope::disabled(),
    }
}

/// Finish summaries that try to reopen exploration / ask for tool access.
pub fn is_escape_finish(summary: &str) -> bool {
    let l = summary.to_ascii_lowercase();
    let patterns = [
        "grant read",
        "grant search",
        "grant permission",
        "please grant",
        "need permission",
        "permissions so i can",
        "need to read",
        "need to search",
        "cannot read",
        "can't read",
        "unable to read",
        "unable to complete the investigation",
        "available tools are restricted",
        "tools are restricted",
        "tools is restricted",
        "read/search",
        "read or search",
        "further investigation",
        "investigation would need",
        "please allow",
        "without read",
        "without search",
        "request read",
        "request search",
    ];
    if patterns.iter().any(|p| l.contains(p)) {
        return true;
    }
    // "restricted to update_plan / apply_patch / finish" style excuses
    l.contains("restricted")
        && (l.contains("update_plan") || l.contains("apply_patch") || l.contains("phase"))
}

fn escalate_invalid(
    run: &mut RunState,
    name: &str,
    reason: InvalidActionReason,
    contract: &PhaseContract,
) -> ActionDecision {
    run.consecutive_invalid_actions = run.consecutive_invalid_actions.saturating_add(1);
    let n = run.consecutive_invalid_actions;
    let max = contract.max_invalid_actions.max(1);

    if n >= max {
        return fallback_decision(run, contract.fallback, name, reason, contract);
    }

    let require_tool_choice = n >= 2;
    if require_tool_choice {
        run.require_tool_choice = true;
    }

    let correction = if name == "finish" && matches!(reason, InvalidActionReason::ArgumentPolicyViolation)
    {
        escape_finish_correction(run, n, require_tool_choice)
    } else {
        let base = correction_message(contract, name, reason, n, require_tool_choice);
        run.act_guidance_with_evidence(&base)
    };
    run.queue_phase_guidance(&correction);

    ActionDecision::RejectAndRetry {
        reason,
        correction,
        require_tool_choice,
    }
}

fn escape_finish_correction(run: &RunState, attempt: usize, require_tool_choice: bool) -> String {
    let mut msg = String::from(
        "ACTION_NOT_AVAILABLE (argument_policy_violation)\n\n\
         finish requesting more tools or permissions is not allowed in Act.\n\
         Exploration is closed. You already have enough context to edit or to \
         name a concrete code-level blocker (file + suspected bug).\n\n\
         Allowed actions: read_file (already-seen files only), update_plan, \
         apply_patch, finish.\n\
         Prefer apply_patch now.",
    );
    if !run.seen_files.is_empty() {
        msg.push_str("\n\nFiles already in context:");
        let mut paths: Vec<_> = run.seen_files.iter().collect();
        paths.sort_by_key(|p| p.to_string_lossy().to_string());
        for path in paths.iter().take(12) {
            msg.push_str(&format!("\n- {}", path.display()));
        }
    }
    if attempt >= 2 || require_tool_choice {
        msg.push_str(
            "\n\nThis is a corrective retry. Your next response MUST call \
             apply_patch, update_plan, or finish with a concrete code blocker \
             (no tool-permission requests).",
        );
    }
    msg
}

fn fallback_decision(
    run: &mut RunState,
    fallback: InvalidFallback,
    name: &str,
    reason: InvalidActionReason,
    contract: &PhaseContract,
) -> ActionDecision {
    let detail = format!(
        "Stopped after {} invalid actions in {} phase (last: `{name}` / {}).",
        run.consecutive_invalid_actions,
        contract.phase_label,
        reason.as_str()
    );

    match fallback {
        InvalidFallback::TransitionToAct => {
            let _ = run.enter_act();
            run.consecutive_invalid_actions = 0;
            let correction = format!(
                "PHASE TRANSITION: ACT\n\n\
                 Too many invalid explore actions. Repository exploration has ended.\n\
                 Available actions: {}.\n\
                 Do not call tools from previous phases.",
                ACT_TOOLS.join(", ")
            );
            run.queue_phase_guidance(&correction);
            ActionDecision::TransitionToAct { correction }
        }
        InvalidFallback::FinishWithBlocker => {
            let message = format!(
                "{detail}\n\
                 Call finish with a blocker, or stop here: no safe edit was made."
            );
            ActionDecision::Stop {
                reason: StopReason::NoProgress,
                message,
            }
        }
        InvalidFallback::FinishUnverified => ActionDecision::Stop {
            reason: StopReason::CompletedUnverified,
            message: format!("{detail}\nFinishing unverified."),
        },
        InvalidFallback::AutoComplete => {
            let reason = if run.entered_finish_turn.is_some() {
                // Prefer TaskComplete only when verify already passed — callers
                // may refine; default CompletedUnverified is safer.
                StopReason::CompletedUnverified
            } else {
                StopReason::CompletedUnverified
            };
            ActionDecision::Stop {
                reason,
                message: format!("{detail}\nAuto-completing finish phase."),
            }
        }
        InvalidFallback::StopNoProgress => ActionDecision::Stop {
            reason: StopReason::NoProgress,
            message: detail,
        },
    }
}

fn correction_message(
    contract: &PhaseContract,
    name: &str,
    reason: InvalidActionReason,
    attempt: usize,
    require_tool_choice: bool,
) -> String {
    let allowed = contract.allowed_tools.join(", ");
    let mut msg = format!(
        "ACTION_NOT_AVAILABLE ({})\n\n\
         The requested action `{name}` is unavailable in the current phase.\n\n\
         Current phase: {}\n\
         Allowed actions:\n",
        reason.as_str(),
        contract.phase_label
    );
    for tool in contract.allowed_tools {
        msg.push_str(&format!("- `{tool}`\n"));
    }
    msg.push_str("\nChoose exactly one allowed action. Do not request tools from other phases.");
    if attempt >= 2 || require_tool_choice {
        msg.push_str(
            "\n\nThis is a corrective retry. Your next response MUST call one of: ",
        );
        msg.push_str(&allowed);
        msg.push('.');
    }
    msg
}

/// Build a ToolResult for RejectAndRetry / Transition decisions.
pub fn denial_tool_result(correction: &str) -> ToolResult {
    ToolResult::error(correction.to_string())
}

fn action_tool_name(action: &Action) -> &'static str {
    match action {
        Action::ReadFile { .. } => "read_file",
        Action::ListFiles { .. } => "list_files",
        Action::SearchCode { .. } => "search_code",
        Action::Locate { .. } => "locate",
        Action::FindFiles { .. } => "find_files",
        Action::DetectProjectSnapshot { .. } => "detect_project_snapshot",
        Action::SummarizeRepo { .. } => "summarize_repo",
        Action::GitStatus => "git_status",
        Action::GitDiff { .. } => "git_diff",
        Action::WriteFile { .. } => "write_file",
        Action::ApplyPatch { .. } => "apply_patch",
        Action::RunCommand { .. } => "run_command",
        Action::RunShell { .. } => "run_shell",
        Action::UpdatePlan { .. } => "update_plan",
        Action::Finish { .. } => "finish",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn read(path: &str) -> Action {
        Action::ReadFile {
            path: PathBuf::from(path),
            offset: 1,
            limit: 50,
        }
    }

    fn idle_recovery() -> RecoveryState {
        RecoveryState::new()
    }

    #[test]
    fn act_read_file_is_invalid_and_escalates() {
        let mut run = RunState::new();
        let recovery = idle_recovery();
        assert!(run.enter_act());
        let _ = run.phase_guidance_for_injection(&recovery); // clear one-shot

        let d1 = validate_action(&mut run, &recovery, &read("Board.tsx"));
        assert!(matches!(d1, ActionDecision::RejectAndRetry { require_tool_choice: false, .. }));
        assert_eq!(run.consecutive_invalid_actions, 1);

        let d2 = validate_action(&mut run, &recovery, &read("Board.tsx"));
        match d2 {
            ActionDecision::RejectAndRetry {
                require_tool_choice: true,
                ..
            } => {}
            other => panic!("expected 2nd retry with require_tool_choice, got {other:?}"),
        }
        assert!(run.require_tool_choice);

        let d3 = validate_action(&mut run, &recovery, &read("Board.tsx"));
        match d3 {
            ActionDecision::Stop {
                reason: StopReason::NoProgress,
                ..
            } => {}
            other => panic!("expected stop on 3rd, got {other:?}"),
        }
    }

    #[test]
    fn act_allows_read_of_previously_seen_file() {
        let mut run = RunState::new();
        let recovery = idle_recovery();
        run.note_file_seen(PathBuf::from("Board.tsx"));
        assert!(run.enter_act());
        let _ = run.phase_guidance_for_injection(&recovery);

        let decision = validate_action(&mut run, &recovery, &read("Board.tsx"));
        assert!(matches!(decision, ActionDecision::Allow));
        assert_eq!(run.consecutive_invalid_actions, 0);
    }

    #[test]
    fn act_rejects_read_outside_seen_files_scope() {
        let mut run = RunState::new();
        let recovery = idle_recovery();
        run.note_file_seen(PathBuf::from("Board.tsx"));
        assert!(run.enter_act());
        let _ = run.phase_guidance_for_injection(&recovery);

        let decision = validate_action(&mut run, &recovery, &read("store.ts"));
        match decision {
            ActionDecision::RejectAndRetry {
                reason: InvalidActionReason::ArgumentPolicyViolation,
                ..
            } => {}
            other => panic!("expected argument policy violation, got {other:?}"),
        }
    }

    #[test]
    fn valid_act_action_resets_counter() {
        let mut run = RunState::new();
        let recovery = idle_recovery();
        let _ = run.enter_act();
        let _ = validate_action(&mut run, &recovery, &read("x.rs"));
        assert_eq!(run.consecutive_invalid_actions, 1);

        let d = validate_action(
            &mut run,
            &recovery,
            &Action::UpdatePlan {
                objective: "fix board".into(),
                steps: "[ ] patch store".into(),
            },
        );
        assert!(matches!(d, ActionDecision::Allow));
        assert_eq!(run.consecutive_invalid_actions, 0);
        assert!(!run.require_tool_choice);
    }

    #[test]
    fn explore_fallback_transitions_to_act() {
        let mut run = RunState::new();
        let recovery = idle_recovery();
        run.consecutive_invalid_actions = 2;
        let contract = contract_for(&run, &recovery);
        assert!(matches!(contract.fallback, InvalidFallback::TransitionToAct));
        let d = escalate_invalid(
            &mut run,
            "bogus_tool",
            InvalidActionReason::ToolUnavailable,
            &contract,
        );
        assert!(matches!(d, ActionDecision::TransitionToAct { .. }));
        assert_eq!(run.phase, Phase::Act);
    }

    #[test]
    fn escape_finish_requesting_tools_is_rejected_in_act() {
        let mut run = RunState::new();
        let recovery = idle_recovery();
        run.note_file_seen(PathBuf::from("Board.tsx"));
        assert!(run.enter_act());
        let _ = run.phase_guidance_for_injection(&recovery);

        let finish = Action::Finish {
            summary: "I was unable to complete the investigation because tools are restricted \
                      to update_plan, apply_patch, and finish. Please grant read/search \
                      permissions so I can complete the fix."
                .into(),
        };
        let d = validate_action(&mut run, &recovery, &finish);
        match d {
            ActionDecision::RejectAndRetry {
                reason: InvalidActionReason::ArgumentPolicyViolation,
                correction,
                ..
            } => {
                assert!(correction.contains("requesting more tools"));
                assert!(correction.contains("Board.tsx"));
            }
            other => panic!("expected escape finish rejection, got {other:?}"),
        }
        assert!(run.act_force_edit);
    }

    #[test]
    fn concrete_code_blocker_finish_is_allowed_in_act() {
        let mut run = RunState::new();
        let recovery = idle_recovery();
        assert!(run.enter_act());
        let finish = Action::Finish {
            summary: "Blocked: Board.tsx fetchTasks likely sends local date while \
                      TaskListCreateView filters UTC midnight — needs product confirm."
                .into(),
        };
        assert!(matches!(
            validate_action(&mut run, &recovery, &finish),
            ActionDecision::Allow
        ));
    }

    #[test]
    fn is_escape_finish_detects_permission_requests() {
        assert!(is_escape_finish(
            "Please grant read permissions so I can continue the investigation."
        ));
        assert!(is_escape_finish(
            "available tools are restricted to update_plan, apply_patch, and finish"
        ));
        assert!(!is_escape_finish(
            "Blocked: store.ts date filter uses startOfDay incorrectly"
        ));
    }

    fn engage_failed_patch(run: &mut RunState, path: &Path) -> RecoveryState {
        let mut recovery = RecoveryState::new();
        let _ = recovery.after_patch_failure(path, ToolResult::error("hunk mismatch"));
        let _ = run.engage_patch_recovery_phase();
        recovery
    }

    #[test]
    fn recovery_wrong_path_is_recovery_mismatch() {
        let mut run = RunState::new();
        let recovery = engage_failed_patch(&mut run, Path::new("a.rs"));
        let d = validate_action(&mut run, &recovery, &read("b.rs"));
        match d {
            ActionDecision::RejectAndRetry {
                reason: InvalidActionReason::RecoveryTargetMismatch,
                ..
            } => {}
            other => panic!("expected recovery mismatch, got {other:?}"),
        }
    }

    #[test]
    fn recovery_exact_path_is_allowed_via_tool_scope() {
        let mut run = RunState::new();
        let recovery = engage_failed_patch(&mut run, Path::new("src/Board.tsx"));
        let d = validate_action(&mut run, &recovery, &read("src/Board.tsx"));
        assert!(matches!(d, ActionDecision::Allow));
    }

    #[test]
    fn recovery_scope_ignores_act_seen_files_evidence() {
        let mut run = RunState::new();
        run.note_file_seen(PathBuf::from("other.rs"));
        let recovery = engage_failed_patch(&mut run, Path::new("target.rs"));
        assert!(matches!(
            validate_action(&mut run, &recovery, &read("target.rs")),
            ActionDecision::Allow
        ));
        match validate_action(&mut run, &recovery, &read("other.rs")) {
            ActionDecision::RejectAndRetry {
                reason: InvalidActionReason::RecoveryTargetMismatch,
                ..
            } => {}
            other => panic!("expected recovery mismatch, got {other:?}"),
        }
    }

    #[test]
    fn patch_recovery_scope_is_disabled_without_a_target() {
        // Defensive only: active recovery without a target should not panic.
        let recovery = RecoveryState::new();
        // Manually impossible via public API; prove disabled fallback for empty target.
        assert!(!patch_recovery_scope(&recovery).permits_path(Path::new("anything.rs")));
    }
}
