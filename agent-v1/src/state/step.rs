use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::tools::ToolResult;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "tool", content = "args")]
pub enum Action {
    ReadFile { path: PathBuf },

    WriteFile { path: PathBuf, content: String },

    RunCommand { cmd: String },

    Finish { summary: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Step {
    pub step_index: u32,

    pub timestamp: String,

    pub action: Action,

    pub result: ToolResult,
}

impl Step {
    pub fn new(step_index: u32, action: Action, result: ToolResult) -> Self {
        Self {
            step_index,
            timestamp: chrono::Utc::now().to_rfc3339(),
            action,
            result,
        }
    }
}
