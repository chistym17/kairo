use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum StopReason {
    TaskComplete,
    /// Finish accepted without a successful verify command (non-benchmark runs).
    CompletedUnverified,
    /// Run stopped by governance after repeated no-progress actions.
    NoProgress,
    MaxStepsExceeded,
    ToolError,
    ModelGaveUp,
}
