//! Runtime governance — progress tracking, deadlock detection, tool gating.
//!
//! Answers only: "Can this next action still produce useful progress?"

mod decide;
mod fingerprint;
mod policy;
mod state;

#[cfg(test)]
mod tests;

pub use decide::{classify_progress, ActionProgress, GovernanceBlockCode, GovernanceDecision};
pub use fingerprint::{
    classify_shell, fingerprint, fingerprint_hash, ActionFingerprint, VerificationFamily,
};
pub use policy::ToolPolicy;
pub use state::{EscalationLevel, GovernanceMetrics, GovernanceState, RuntimeMode};

pub const BLOCKED_REPEAT_THRESHOLD: u32 = 3;
pub const STAGNATION_TURNS_AFTER_EDIT: u32 = 8;
pub const VERIFY_FAMILY_FAILURE_THRESHOLD: u32 = 3;
pub const REVIEW_MODE_VIOLATIONS_BEFORE_TERMINATE: u32 = 3;
