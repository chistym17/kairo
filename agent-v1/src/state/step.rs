use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::tools::ToolResult;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "tool", content = "args")]
pub enum Action {
    ReadFile { path: PathBuf },

    ListFiles { path: PathBuf },

    /// Case-insensitive content search. `path` scopes the search (default `.`).
    SearchCode { query: String, path: PathBuf },

    /// Glob file search (e.g. `*.tsx`, `**/routes/*`). `path` scopes the search.
    FindFiles { pattern: String, path: PathBuf },

    GitStatus,

    /// Working-tree diff vs HEAD. Optional path scopes the diff.
    GitDiff { path: Option<PathBuf> },

    WriteFile { path: PathBuf, content: String },

    ApplyPatch { path: PathBuf, diff: String },

    RunCommand { cmd: String },

    Finish { summary: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Step {
    pub step_index: u32,

    pub timestamp: String,

    pub action: Action,

    /// Provider tool_call_id — must match between assistant tool_calls and tool result.
    pub tool_call_id: String,

    pub result: ToolResult,
}

impl Step {
    pub fn new(
        step_index: u32,
        action: Action,
        result: ToolResult,
        tool_call_id: impl Into<String>,
    ) -> Self {
        Self {
            step_index,
            timestamp: chrono::Utc::now().to_rfc3339(),
            action,
            tool_call_id: tool_call_id.into(),
            result,
        }
    }
}
