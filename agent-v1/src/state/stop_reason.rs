use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum StopReason {
    TaskComplete,
    /// Finish accepted without a successful verify command (non-benchmark runs).
    CompletedUnverified,
    MaxStepsExceeded,
    ToolError,
    ModelGaveUp,
}
