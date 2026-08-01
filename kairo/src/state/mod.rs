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
pub mod stall;
pub mod step;
pub mod stop_reason;
pub mod tool_scope;
pub mod workspace_summary;

pub use agent_state::AgentState;
pub use action_policy::{
    validate_action, ActionDecision, InvalidActionReason, InvalidFallback, PhaseContract,
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
    RecoveryReason, RecoverySnapshot, RecoveryState, MAX_CONSECUTIVE_PATCH_FAILURES,
    MAX_DENIED_SOURCE_WRITES,
};
pub use stall::{ActionFingerprint, StallKind, StallState};
pub use step::{Action, Step};
pub use stop_reason::StopReason;
