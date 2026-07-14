use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::state::step::Action;
use crate::state::stop_reason::StopReason;

pub mod apply_patch;
pub mod diff;
pub mod find_files;
pub mod git_diff;
pub mod git_status;
pub mod ignore;
pub mod list_files;
pub mod read_file;
pub mod run_command;
pub mod search_code;
pub mod write_file;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub success: bool,
    pub output: String,
    pub diff: Option<String>,
    pub error: Option<String>,
    pub stop_reason: Option<StopReason>,
}

impl ToolResult {
    /// Failed tool outcome that the model can observe and recover from.
    /// Does **not** stop the agent loop (Pi-like: ordinary failures are observations).
    pub fn error(msg: impl Into<String>) -> Self {
        Self {
            success: false,
            output: String::new(),
            diff: None,
            error: Some(msg.into()),
            stop_reason: None,
        }
    }
}

const MAX_OUTPUT_CHARS: usize = 2000;

pub fn truncate_output(raw: &str) -> String {
    let stripped = strip_ansi(raw);
    if stripped.len() <= MAX_OUTPUT_CHARS {
        stripped
    } else {
        format!(
            "{}\n...[truncated — {} total chars]",
            &stripped[..MAX_OUTPUT_CHARS],
            stripped.len()
        )
    }
}

fn strip_ansi(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for ch in chars.by_ref() {
                    if ch.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            result.push(c);
        }
    }

    result
}
pub fn dispatch(action: &Action, workspace_root: &Path) -> ToolResult {
    match action {
        Action::ReadFile {
            path,
            offset,
            limit,
        } => read_file::execute(path, *offset, *limit, workspace_root),

        Action::ListFiles { path } => list_files::execute(path, workspace_root),

        Action::SearchCode { query, path } => search_code::execute(query, path, workspace_root),

        Action::FindFiles { pattern, path } => find_files::execute(pattern, path, workspace_root),

        Action::GitStatus => git_status::execute(workspace_root),

        Action::GitDiff { path } => git_diff::execute(path, workspace_root),

        Action::WriteFile { path, content } => write_file::execute(path, content, workspace_root),

        Action::ApplyPatch { path, diff } => apply_patch::execute(path, diff, workspace_root),

        Action::RunCommand {
            cmd,
            cwd,
            timeout_secs,
        } => run_command::execute(
            run_command::RunCommandOpts {
                cmd,
                cwd,
                timeout_secs: *timeout_secs,
            },
            workspace_root,
        ),

        Action::UpdatePlan { .. } => ToolResult::error(
            "update_plan must be handled by the loop engine (internal error)",
        ),

        Action::Finish { summary } => ToolResult {
            success: true,
            output: summary.clone(),
            diff: None,
            error: None,
            stop_reason: Some(StopReason::TaskComplete),
        },
    }
}
