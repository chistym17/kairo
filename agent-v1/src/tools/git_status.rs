use std::path::Path;
use std::process::Command;

use crate::tools::{truncate_output, ToolResult};

/// Show git status for the workspace (branch + porcelain file list).
pub fn execute(workspace_root: &Path) -> ToolResult {
    if !workspace_root.exists() {
        return ToolResult::error(format!(
            "workspace_root does not exist: {}",
            workspace_root.display()
        ));
    }

    run_git(workspace_root, &["status", "--short", "--branch"])
}

pub(crate) fn run_git(workspace_root: &Path, args: &[&str]) -> ToolResult {
    let output = Command::new("git")
        .args(args)
        .current_dir(workspace_root)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", std::env::var("HOME").unwrap_or_default())
        .env(
            "GIT_TERMINAL_PROMPT",
            std::env::var("GIT_TERMINAL_PROMPT").unwrap_or_else(|_| "0".to_string()),
        )
        .output();

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            if out.status.success() {
                let body = if stdout.trim().is_empty() {
                    "(clean — no changes)".to_string()
                } else {
                    stdout.trim_end().to_string()
                };
                ToolResult {
                    success: true,
                    output: truncate_output(&body),
                    diff: None,
                    error: None,
                    stop_reason: None,
                }
            } else {
                let msg = if !stderr.trim().is_empty() {
                    stderr.trim().to_string()
                } else if !stdout.trim().is_empty() {
                    stdout.trim().to_string()
                } else {
                    format!("git {:?} failed with status {}", args, out.status)
                };
                ToolResult::error(msg)
            }
        }
        Err(e) => ToolResult::error(format!(
            "failed to spawn git: {e}. Is git installed and on PATH?"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;

    #[test]
    fn status_in_git_repo() {
        let dir = tempfile::tempdir().unwrap();
        let init = Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .expect("git init");
        assert!(init.status.success(), "{:?}", String::from_utf8_lossy(&init.stderr));

        // Identify for commit-less status
        let _ = Command::new("git")
            .args(["config", "user.email", "test@example.com"])
            .current_dir(dir.path())
            .output();
        let _ = Command::new("git")
            .args(["config", "user.name", "test"])
            .current_dir(dir.path())
            .output();

        fs::write(dir.path().join("hello.txt"), "hi").unwrap();
        let result = execute(dir.path());
        assert!(result.success, "{:?}", result.error);
        assert!(
            result.output.contains("hello.txt") || result.output.contains("##"),
            "{}",
            result.output
        );
    }

    #[test]
    fn status_outside_git_errors() {
        let dir = tempfile::tempdir().unwrap();
        let result = execute(dir.path());
        assert!(!result.success);
    }
}
