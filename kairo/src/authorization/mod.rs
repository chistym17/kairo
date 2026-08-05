//! Tool authorization gate (Phase 12).
//!
//! Classify tool effects, authorize against [`EditPolicy`], and optionally
//! pause for user approval. Approvals are capability-based — not patch-only.

mod gate;
mod live;
mod types;

pub use gate::{ApprovalGate, AutoApprovalGate, StdinApprovalGate};
pub use live::LiveControls;
pub use types::{
    apply_decision, authorize, build_approval_request, tool_effect, ApprovalDecision,
    ApprovalId, ApprovalRequest, AuthorizationDecision, AuthorizationState, EditPolicy, Scope,
    ToolEffect,
};
