//! Stall-policy feature flag and candidate unified decisions (Slice 4 Part 5).
//!
//! **Not a destructive cutover.** Legacy counters remain authoritative for
//! Explore→Act, denial stops, invalid-action fallback, and progress nudges.
//! This module computes what a *candidate* unified stall policy would decide
//! on each turn so corpus runs can compare without changing runtime behavior
//! (default: [`StallPolicyMode::Shadow`]).
//!
//! ## `KAIRO_STALL_POLICY`
//!
//! | Value | Behavior |
//! |---|---|
//! | `shadow` (default) | Log candidate; legacy + Part 3/4 recipes control the run |
//! | `legacy` | Same control as shadow; still logs candidate for diffing |
//! | `unified` | Additionally apply provisional `consecutive_stalls` terminate |
//!
//! Part 4 same-key / ping-pong recipe termination stays live in all modes
//! (already approved). Unified only adds the *mixed-stall* threshold that
//! has not been corpus-justified yet.
//!
//! ## Legacy field → replacement map (do not delete until each row is done)
//!
//! | Legacy field | Current decisions | Candidate / planned replacement |
//! |---|---|---|
//! | `run_control.no_progress` | Explore → Act | `transition_policy` from stall / derived action policy |
//! | `run_control.consecutive_denials` | general stop @ 6 | unified stall terminate (kind + history) |
//! | `run_control.consecutive_invalid_actions` | correction; Verify/Finish stop | Act/patch_recovery defer stop to Layer 4 recipes |
//! | `progress.consecutive_no_progress` | nudges / checkpoint | context snapshot + unified stall policy |
//!
//! Subsystem budgets that must **never** fold into the general counter:
//! Recovery (FailedPatch), Verification, Authorization.

use serde_json::json;

use crate::state::agent_state::AgentState;
use crate::state::runtime_decision::{RecoveryAction, RuntimeTransition, TurnOutcome};
use crate::state::stall::StallKind;

/// Mixed general stalls: candidate would keep recovering at this count.
pub const MIXED_STALL_RECOVER_AT: u32 = 4;

/// Mixed general stalls: candidate would terminate at this count (provisional).
pub const MIXED_STALL_TERMINATE_AT: u32 = 7;

/// Which stall-policy mode is active for this process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StallPolicyMode {
    /// Candidate logged; never applied beyond approved Part 3/4 recipes.
    Legacy,
    /// Candidate logged; legacy counters + Part 3/4 remain authoritative.
    Shadow,
    /// Candidate mixed-stall terminate threshold may also stop the run.
    Unified,
}

impl StallPolicyMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Legacy => "legacy",
            Self::Shadow => "shadow",
            Self::Unified => "unified",
        }
    }

    /// Read `KAIRO_STALL_POLICY`. Unset / unknown → [`StallPolicyMode::Shadow`].
    pub fn from_env() -> Self {
        match std::env::var("KAIRO_STALL_POLICY") {
            Ok(v) => match v.trim().to_ascii_lowercase().as_str() {
                "legacy" => Self::Legacy,
                "unified" => Self::Unified,
                "shadow" => Self::Shadow,
                _ => Self::Shadow,
            },
            Err(_) => Self::Shadow,
        }
    }

    /// Whether provisional mixed-stall terminate (`consecutive_stalls >= 7`)
    /// may stop the run. Part 4 recipe stops are independent of this flag.
    pub fn applies_mixed_stall_terminate(self) -> bool {
        matches!(self, Self::Unified)
    }
}

/// One candidate decision for corpus comparison (Part 5A).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateStallDecision {
    pub decision: &'static str,
    pub reason: String,
    pub consecutive_stalls: u32,
    pub stall_kind: Option<StallKind>,
}

impl CandidateStallDecision {
    pub fn continue_run(stalls: u32) -> Self {
        Self {
            decision: "continue",
            reason: "no_stall".into(),
            consecutive_stalls: stalls,
            stall_kind: None,
        }
    }
}

/// Compute what the *candidate* unified policy would decide this turn.
///
/// Pure w.r.t. control: does not mutate state. Uses post-observe stall
/// counters and the Layer 4 transition already selected (including Part 4
/// terminate after `apply_recovery` when `layer4` is `Terminate`).
pub fn compute_candidate_decision(
    state: &AgentState,
    outcome: &TurnOutcome,
    layer4: &RuntimeTransition,
) -> CandidateStallDecision {
    let stalls = state.stall.consecutive_stalls;
    let kind = outcome.stall_kind;

    match layer4 {
        RuntimeTransition::Terminate(reason) => {
            return CandidateStallDecision {
                decision: "terminate",
                reason: format!("layer4_{reason:?}"),
                consecutive_stalls: stalls,
                stall_kind: kind,
            };
        }
        RuntimeTransition::Complete(reason) => {
            return CandidateStallDecision {
                decision: "complete",
                reason: format!("{reason:?}"),
                consecutive_stalls: stalls,
                stall_kind: kind,
            };
        }
        RuntimeTransition::Recover(action) => {
            let reason = recipe_reason(action);
            if stalls >= MIXED_STALL_TERMINATE_AT {
                return CandidateStallDecision {
                    decision: "terminate",
                    reason: format!("mixed_stall_terminate_at_{MIXED_STALL_TERMINATE_AT}"),
                    consecutive_stalls: stalls,
                    stall_kind: kind,
                };
            }
            // Explore → Act is still owned by `run_control.no_progress`; surface
            // the candidate replacement only when not already past terminate.
            if matches!(state.run_control.phase, crate::state::phase::Phase::Explore)
                && stalls >= 2
            {
                return CandidateStallDecision {
                    decision: "transition_policy",
                    reason: "explore_act_via_consecutive_stalls".into(),
                    consecutive_stalls: stalls,
                    stall_kind: kind,
                };
            }
            return CandidateStallDecision {
                decision: "recover",
                reason,
                consecutive_stalls: stalls,
                stall_kind: kind,
            };
        }
        RuntimeTransition::Continue => {}
    }

    if let Some(kind) = kind {
        if stalls >= MIXED_STALL_TERMINATE_AT {
            return CandidateStallDecision {
                decision: "terminate",
                reason: format!("mixed_stall_terminate_at_{MIXED_STALL_TERMINATE_AT}"),
                consecutive_stalls: stalls,
                stall_kind: Some(kind),
            };
        }
        if matches!(state.run_control.phase, crate::state::phase::Phase::Explore) && stalls >= 2
        {
            return CandidateStallDecision {
                decision: "transition_policy",
                reason: "explore_act_via_consecutive_stalls".into(),
                consecutive_stalls: stalls,
                stall_kind: Some(kind),
            };
        }
        if stalls >= MIXED_STALL_RECOVER_AT {
            return CandidateStallDecision {
                decision: "recover",
                reason: format!("mixed_stall_recover_at_{MIXED_STALL_RECOVER_AT}"),
                consecutive_stalls: stalls,
                stall_kind: Some(kind),
            };
        }
        return CandidateStallDecision {
            decision: "recover",
            reason: format!("stall_{kind:?}"),
            consecutive_stalls: stalls,
            stall_kind: Some(kind),
        };
    }

    CandidateStallDecision::continue_run(stalls)
}

fn recipe_reason(action: &RecoveryAction) -> String {
    match action {
        RecoveryAction::FailedPatch { .. } => "failed_patch".into(),
        RecoveryAction::InvalidArguments { .. } => "invalid_arguments".into(),
        RecoveryAction::UnsupportedCapability { cause, .. } => {
            format!("unsupported_{cause:?}")
        }
        RecoveryAction::MissingEvidence { relevance, .. } => {
            format!("missing_evidence_{}", relevance.as_str())
        }
        RecoveryAction::RepeatedVerification => "repeated_verification".into(),
    }
}

/// Map the authoritative (legacy + Part 3/4) outcome to a short label.
pub fn legacy_decision_label(
    stop: Option<&crate::state::stop_reason::StopReason>,
    recovered: bool,
) -> &'static str {
    if stop.is_some() {
        "terminate"
    } else if recovered {
        "recover"
    } else {
        "continue"
    }
}

/// Log Part 5A shadow record. Always safe to call; does not change control.
pub fn log_candidate_decision(
    state: &AgentState,
    mode: StallPolicyMode,
    legacy_decision: &str,
    candidate: &CandidateStallDecision,
) {
    let payload = json!({
        "policy": mode.as_str(),
        "legacy_decision": legacy_decision,
        "candidate_decision": candidate.decision,
        "candidate_reason": candidate.reason,
        "consecutive_stalls": candidate.consecutive_stalls,
        "stall_kind": candidate.stall_kind.map(|k| format!("{k:?}")),
        "legacy": {
            "no_progress": state.run_control.no_progress,
            "consecutive_denials": state.run_control.consecutive_denials,
            "consecutive_invalid_actions": state.run_control.consecutive_invalid_actions,
            "progress_consecutive_no_progress": state.progress.consecutive_no_progress,
        },
        "fingerprints": {
            "exact_repeat": state.stall.is_repeat(),
            "coarse_repeat": state.stall.coarse_is_repeat(),
            "ping_pong": state.stall.is_ping_pong(),
        },
    });
    crate::logging::write_event(
        &state.run_id,
        "stall_policy_shadow",
        &payload.to_string(),
        0,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::runtime_decision::TurnOutcome;
    use std::path::PathBuf;

    fn state() -> AgentState {
        AgentState::new("goal", PathBuf::from("/tmp"), "test-model", 10)
    }

    #[test]
    fn default_policy_is_shadow() {
        // Do not rely on process env being unset in parallel tests — just
        // check the parse paths.
        assert_eq!(StallPolicyMode::Shadow.as_str(), "shadow");
        assert!(!StallPolicyMode::Shadow.applies_mixed_stall_terminate());
        assert!(StallPolicyMode::Unified.applies_mixed_stall_terminate());
        assert!(!StallPolicyMode::Legacy.applies_mixed_stall_terminate());
    }

    #[test]
    fn candidate_terminates_at_mixed_threshold() {
        let mut s = state();
        let _ = s.run_control.enter_act();
        s.stall.consecutive_stalls = MIXED_STALL_TERMINATE_AT;
        let outcome = TurnOutcome {
            stop_reason: None,
            stall_kind: Some(StallKind::DuplicateEvidence),
            dispatched: true,
            tool_name: "read_file".into(),
            target_path: Some(PathBuf::from("a.ts")),
        };
        let c = compute_candidate_decision(&s, &outcome, &RuntimeTransition::Continue);
        assert_eq!(c.decision, "terminate");
        assert!(c.reason.contains("mixed_stall_terminate"));
    }

    #[test]
    fn candidate_recover_below_terminate() {
        let mut s = state();
        let _ = s.run_control.enter_act();
        s.stall.consecutive_stalls = MIXED_STALL_RECOVER_AT;
        let outcome = TurnOutcome {
            stop_reason: None,
            stall_kind: Some(StallKind::ScopeViolation),
            dispatched: false,
            tool_name: "search_code".into(),
            target_path: None,
        };
        let c = compute_candidate_decision(&s, &outcome, &RuntimeTransition::Continue);
        assert_eq!(c.decision, "recover");
    }
}
