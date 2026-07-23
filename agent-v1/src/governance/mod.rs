//! Runtime governance — progress tracking, deadlock detection, tool gating.
//!
//! **Default off.** Semantic policy (EditOrJustify / ReviewOrFinish / tool removal)
//! is disabled unless `SEMANTIC_POLICY=true`. Kept for comparison; see phasev2.md.

mod decide;
mod fingerprint;
mod policy;
mod pre_edit;
mod state;

#[cfg(test)]
mod tests;

pub use decide::{classify_progress, ActionProgress, GovernanceBlockCode, GovernanceDecision};
pub use fingerprint::{
    classify_shell, fingerprint, fingerprint_hash, verification_family_for_command,
    ActionFingerprint, VerificationFamily,
};
pub use policy::ToolPolicy;
pub use pre_edit::{PreEditTracker, PRE_EDIT_STAGNATION_WINDOW};
pub use state::{EscalationLevel, GovernanceMetrics, GovernanceState, RuntimeMode};

pub const BLOCKED_REPEAT_THRESHOLD: u32 = 3;
pub const STAGNATION_TURNS_AFTER_EDIT: u32 = 8;
pub const VERIFY_FAMILY_FAILURE_THRESHOLD: u32 = 3;
pub const REVIEW_MODE_VIOLATIONS_BEFORE_TERMINATE: u32 = 3;
/// Model turns without edit at which each no-edit nudge fires (max 4).
/// Only applied when `semantic_policy` is enabled.
pub const NO_EDIT_NUDGE_THRESHOLDS: [u32; 4] = [4, 6, 8, 10];
pub const POST_VERIFY_FINISH_TURNS: u32 = 2;
pub const MAX_NO_TOOL_TURNS: u32 = 2;
/// After a green verify, this many text-only turns auto-`TaskComplete` (finish avoidance).
pub const AUTO_FINISH_AFTER_VERIFY_NO_TOOL: u32 = 2;
pub const KEEP_RECENT_STEPS: usize = 10;
