use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::tools::ToolResult;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "tool", content = "args")]
pub enum Action {
    ReadFile {
        path: PathBuf,
        /// 1-based start line (default 1).
        offset: u32,
        /// Max lines to return (default 250).
        limit: u32,
    },

    ListFiles { path: PathBuf },

    /// Case-insensitive content search. `path` scopes the search (default `.`).
    SearchCode { query: String, path: PathBuf },

    /// Ranked symbol/concept location (thin repo intelligence). `path` scopes search.
    Locate { query: String, path: PathBuf },

    /// Glob file search (e.g. `*.tsx`, `**/routes/*`). `path` scopes the search.
    FindFiles { pattern: String, path: PathBuf },

    /// Compact orientation map of project units (manifests → kind/commands/entry points).
    DetectProjectSnapshot { path: PathBuf },

    /// Bounded index-backed repository overview (no recursive tree reads).
    SummarizeRepo {
        /// overview | structural | detailed
        depth: String,
        /// Directory / path scope (default `.`).
        path: PathBuf,
        /// Optional focus keywords.
        focus: Option<String>,
    },

    GitStatus,

    /// Working-tree diff vs HEAD. Optional path scopes the diff.
    GitDiff { path: Option<PathBuf> },

    WriteFile { path: PathBuf, content: String },

    ApplyPatch { path: PathBuf, diff: String },

    RunCommand {
        cmd: String,
        /// Relative working directory under the workspace (default `.`).
        cwd: PathBuf,
        /// Timeout in seconds (default 30, max 900).
        timeout_secs: u64,
    },

    /// Real shell (`sh -c`) — allows pipes, &&, venv paths. Prefer for verify workflows.
    RunShell {
        command: String,
        cwd: PathBuf,
        timeout_secs: u64,
    },

    /// Replace the current working plan (objective + checklist).
    UpdatePlan { objective: String, steps: String },

    Finish { summary: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Step {
    pub step_index: u32,

    /// Model turn that produced this tool call (one turn may yield many steps).
    pub model_turn: u32,

    pub timestamp: String,

    pub action: Action,

    /// Provider tool_call_id — must match between assistant tool_calls and tool result.
    pub tool_call_id: String,

    pub result: ToolResult,
}

impl Step {
    pub fn new(
        step_index: u32,
        model_turn: u32,
        action: Action,
        result: ToolResult,
        tool_call_id: impl Into<String>,
    ) -> Self {
        Self {
            step_index,
            model_turn,
            timestamp: chrono::Utc::now().to_rfc3339(),
            action,
            tool_call_id: tool_call_id.into(),
            result,
        }
    }
}
