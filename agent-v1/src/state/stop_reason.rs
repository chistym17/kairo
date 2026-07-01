use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum StopReason {
    TaskComplete,
    MaxStepsExceeded,
    ToolError,
    ModelGaveUp,
}
