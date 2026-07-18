use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

use crate::state::step::Action;
use crate::state::stop_reason::StopReason;

pub mod apply_patch;
pub mod diff;
pub mod find_files;
pub mod git_diff;
pub mod git_status;
pub mod ignore;
pub mod list_files;
pub mod project_snapshot;
pub mod read_cache;
pub mod read_file;
pub mod run_command;
pub mod run_shell;
pub mod search_code;
pub mod shell_cache;
pub mod shell_read;
pub mod symbol_expand;
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
    /// Does **not** stop the agent loop (ordinary failures are observations).
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

/// Resolve a relative path as a directory under `workspace_root`.
/// Canonicalizes both sides and rejects absolute paths, `..`, and escapes
/// (including via symlinks that leave the workspace).
pub fn resolve_workspace_cwd(
    workspace_root: &Path,
    cwd: &Path,
) -> Result<(PathBuf /* canonical abs */, PathBuf /* display rel */), String> {
    let cwd_rel = if cwd.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        cwd.to_path_buf()
    };

    if cwd_rel.is_absolute() {
        return Err(format!(
            "working_directory must be relative (not absolute): {}",
            cwd_rel.display()
        ));
    }
    if cwd_rel
        .components()
        .any(|c| matches!(c, Component::ParentDir))
    {
        return Err(format!(
            "working_directory must not contain '..': {}",
            cwd_rel.display()
        ));
    }

    let canonical_root = workspace_root.canonicalize().map_err(|e| {
        format!(
            "workspace_root could not be resolved ({}): {e}",
            workspace_root.display()
        )
    })?;

    let joined = workspace_root.join(&cwd_rel);
    if !joined.exists() {
        return Err(format!(
            "working_directory not found: {} (under workspace {})",
            cwd_rel.display(),
            workspace_root.display()
        ));
    }
    if !joined.is_dir() {
        return Err(format!(
            "working_directory is not a directory: {}",
            cwd_rel.display()
        ));
    }

    let canonical_cwd = joined.canonicalize().map_err(|e| {
        format!(
            "working_directory could not be resolved ({}): {e}",
            cwd_rel.display()
        )
    })?;

    if !canonical_cwd.starts_with(&canonical_root) {
        return Err(format!(
            "working_directory escape attempt: {} resolves outside workspace {}",
            cwd_rel.display(),
            workspace_root.display()
        ));
    }

    Ok((canonical_cwd, cwd_rel))
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

        Action::Locate { query, path } => crate::repo_intel::execute(query, path, workspace_root),

        Action::FindFiles { pattern, path } => find_files::execute(pattern, path, workspace_root),

        Action::DetectProjectSnapshot { path } => {
            project_snapshot::execute(path, workspace_root)
        }

        Action::GitStatus => git_status::execute(workspace_root),

        Action::GitDiff { path } => git_diff::execute(path, workspace_root, None),

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

        Action::RunShell {
            command,
            cwd,
            timeout_secs,
        } => run_shell::execute(
            run_shell::RunShellOpts {
                command,
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
