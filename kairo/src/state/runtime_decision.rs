//! Layer 4 runtime decision (final-design-plan.md §3, §6, §8 Slice 4 Part 2).
//!
//! **Part 2 scope:** introduce the closed-loop types and a single
//! [`decide_next_state`] entry point that currently maps an already-committed
//! tool outcome onto [`RuntimeTransition::Continue`] /
//! [`RuntimeTransition::Terminate`] **1:1 with today's `ToolResult.stop_reason`
//! handling**. Layer 1 mutations (stall, recovery, phase, progress, …) still
//! happen in `loop_engine::commit_tool_outcome` before this is consulted —
//! Part 3 wires `Recover`, and later parts move the writes themselves here so
//! this becomes the sole Layer 1 mutator as the architecture requires.
//!
//! `Recover` / `Complete` variants exist so Part 3+ can fill them without
//! reshaping the enum; they are not produced by [`decide_next_state`] yet.

use crate::state::agent_state::AgentState;
use crate::state::stop_reason::StopReason;

/// What happened this turn, as seen by Layer 4 after Layer 3 finished.
///
/// Part 2 only carries the committed tool result's stop signal. Later parts
/// will enrich this with classified stall kind / recovery engagement so
/// recipes can choose `Recover` without re-deriving those facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnOutcome {
    /// A tool outcome was recorded (success, failure, or pre-dispatch reject
    /// committed as a tool result). `stop_reason` is whatever the existing
    /// subsystem paths already stamped on the `ToolResult` this turn.
    ToolCommitted { stop_reason: Option<StopReason> },
}

/// Bounded recovery recipe to apply on the next turn (final-design-plan.md §7.2).
///
/// Part 2 declares the variants so Part 3 can wire them without renaming;
/// [`decide_next_state`] never returns `RuntimeTransition::Recover` yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryAction {
    /// Failed patch → one exact reread of the target, then patch/finish only.
    FailedPatch,
    /// Invalid arguments → schema error + one same-tool retry, then different action.
    InvalidArguments,
    /// Unsupported / out-of-scope capability → evidence summary; require edit/plan/finish.
    UnsupportedCapability,
    /// Missing evidence for a known-relevant unseen file → one exact read grant.
    MissingEvidence,
    /// Repeated verification → return cached result; require patch/finish next.
    RepeatedVerification,
}

/// Why the run may complete without a hard failure (Slice 5/6 will own these).
///
/// Part 2 does not yet distinguish `Complete` from `Terminate` — completion-
/// shaped [`StopReason`]s still travel through `Terminate` so write_summary /
/// observer behavior is unchanged.
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

/// Decide the next runtime transition from an already-committed turn outcome.
///
/// Pure with respect to Layer 1 today: reads `outcome` (and later `state`) and
/// returns a transition. Does **not** mutate `state` in Part 2.
pub fn decide_next_state(_state: &AgentState, outcome: &TurnOutcome) -> RuntimeTransition {
    match outcome {
        TurnOutcome::ToolCommitted {
            stop_reason: Some(reason),
        } => RuntimeTransition::Terminate(reason.clone()),
        TurnOutcome::ToolCommitted { stop_reason: None } => RuntimeTransition::Continue,
    }
}

/// Collapse a Part 2 transition back to the `Option<StopReason>` the agent
/// loop already understands. `Recover` / `Complete` are not produced yet;
/// hitting them is a programming error in Part 2.
pub fn stop_reason_from_transition(transition: RuntimeTransition) -> Option<StopReason> {
    match transition {
        RuntimeTransition::Continue => None,
        RuntimeTransition::Terminate(reason) => Some(reason),
        RuntimeTransition::Recover(action) => {
            panic!(
                "decide_next_state returned Recover({action:?}) before Slice 4 Part 3 wired recipes"
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
    use std::path::PathBuf;

    fn empty_state() -> AgentState {
        AgentState::new("goal", PathBuf::from("/tmp"), "test-model", 10)
    }

    #[test]
    fn no_stop_reason_continues() {
        let state = empty_state();
        let t = decide_next_state(
            &state,
            &TurnOutcome::ToolCommitted { stop_reason: None },
        );
        assert_eq!(t, RuntimeTransition::Continue);
        assert!(stop_reason_from_transition(t).is_none());
    }

    #[test]
    fn any_stop_reason_terminates_one_to_one() {
        let state = empty_state();
        for reason in [
            StopReason::NoProgress,
            StopReason::TaskComplete,
            StopReason::CompletedUnverified,
            StopReason::ToolError,
            StopReason::MaxStepsExceeded,
            StopReason::ModelGaveUp,
            StopReason::Cancelled,
        ] {
            let t = decide_next_state(
                &state,
                &TurnOutcome::ToolCommitted {
                    stop_reason: Some(reason.clone()),
                },
            );
            assert_eq!(t, RuntimeTransition::Terminate(reason.clone()));
            assert_eq!(stop_reason_from_transition(t), Some(reason));
        }
    }

    #[test]
    fn part2_does_not_emit_recover_or_complete() {
        let state = empty_state();
        let t = decide_next_state(
            &state,
            &TurnOutcome::ToolCommitted {
                stop_reason: Some(StopReason::TaskComplete),
            },
        );
        assert!(matches!(t, RuntimeTransition::Terminate(_)));
        assert!(!matches!(
            t,
            RuntimeTransition::Recover(_) | RuntimeTransition::Complete(_)
        ));
    }
}
