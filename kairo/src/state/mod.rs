pub mod agent_state;
pub mod action_policy;
pub mod completion;
pub mod efficiency;
pub mod exploration;
pub mod finish_summary;
pub mod phase;
pub mod plan;
pub mod progress;
pub mod recovery;
pub mod runtime_decision;
pub mod stall;
pub mod stall_policy;
pub mod step;
pub mod stop_reason;
pub mod tool_scope;
pub mod workspace_summary;

pub use agent_state::AgentState;
pub use action_policy::{
    allowed_next_actions, validate_action, ActionConstraint, ActionDecision,
    AllowedActionSnapshot, InvalidActionReason, InvalidFallback, PhaseContract,
};
pub use completion::CompletionTracker;
pub use efficiency::RunEfficiency;
pub use exploration::ExplorationTracker;
pub use finish_summary::ground_finish_summary;
pub use phase::{
    Phase, RunState, ACT_STICKY_MESSAGE, ACT_TRANSITION_MESSAGE, FINISH_PHASE_IDLE_TURNS,
    FINISH_TOOLS, FINISH_UNVERIFIED_MESSAGE, FINISH_VERIFIED_MESSAGE, NO_PROGRESS_ACT_THRESHOLD,
    NO_PROGRESS_STOP_THRESHOLD, PATCH_RECOVERY_MESSAGE, PATCH_RECOVERY_TOOLS, VERIFY_TOOLS,
    VERIFY_TRANSITION_MESSAGE,
};
pub use plan::{AgentPlan, PlanConstraint, PlanStatus, PlanStep};
pub use progress::ProgressState;
pub use recovery::{
    build_grounded_patch_packet, patch_cites_grounded_line, GroundedPatchPacket, RecoveryKey,
    RecoveryReason, RecoverySnapshot, RecoveryState, MAX_CONSECUTIVE_PATCH_FAILURES,
    MAX_DENIED_SOURCE_WRITES,
};
pub use runtime_decision::{
    apply_recovery, decide_next_state, stop_reason_from_transition, CompletionReason,
    NoToolCallCause, RecoveryAction, RecoveryApplyResult, RelevanceRule, RuntimeTransition,
    TurnOutcome, UnsupportedCause,
};
pub use stall::{ActionFingerprint, StallKind, StallState};
pub use stall_policy::{
    compute_candidate_decision, legacy_decision_label, log_candidate_decision, StallPolicyMode,
    MIXED_STALL_RECOVER_AT, MIXED_STALL_TERMINATE_AT,
};
pub use step::{Action, Step};
pub use stop_reason::StopReason;
