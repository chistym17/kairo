use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum StopReason {
    TaskComplete,
    /// Finish without green verify, or objective post-edit idle stop.
    CompletedUnverified,
    /// Run stopped by governance after repeated no-progress actions.
    NoProgress,
    MaxStepsExceeded,
    ToolError,
    ModelGaveUp,
    /// Run stopped because the host called [`crate::api::AgentSession::request_stop`].
    Cancelled,
}
