use std::path::Path;
use std::process::Command;
use std::time::Duration;

use crate::state::stop_reason::StopReason;
use crate::tools::{truncate_output, ToolResult};

const COMMAND_TIMEOUT_SECS: u64 = 30;

const ALLOWED_COMMANDS: &[&str] = &[
    "python", "python3", "pytest", "cargo", "npm", "node", "cat", "ls",
];

pub fn execute(cmd: &str, workspace_root: &Path) -> ToolResult {
    if !workspace_root.exists() {
        return ToolResult::error(format!(
            "workspace_root does not exist: {}",
            workspace_root.display()
        ));
    }

    let parts: Vec<&str> = cmd.split_whitespace().collect();
    if parts.is_empty() {
        return ToolResult {
            success: false,
            output: String::new(),
            diff: None,
            error: Some("empty command".to_string()),
            stop_reason: None,
        };
    }

    let program = parts[0];
    let args = &parts[1..];

    if !is_allowed(program) {
        return ToolResult {
            success: false,
            output: String::new(),
            diff: None,
            error: Some(format!(
                "command not allowed: '{}'. Allowed commands: {}",
                program,
                ALLOWED_COMMANDS.join(", ")
            )),
            stop_reason: None,
        };
    }

    let output = Command::new(program)
        .args(args)
        .current_dir(workspace_root)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", std::env::var("HOME").unwrap_or_default())
        .spawn();

    let mut child = match output {
        Ok(c) => c,
        Err(e) => {
            return ToolResult::error(format!(
                "failed to spawn '{}': {}. Is it installed?",
                program, e
            ))
        }
    };

    let timeout = Duration::from_secs(COMMAND_TIMEOUT_SECS);
    let start = std::time::Instant::now();

    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    return ToolResult {
                        success: false,
                        output: String::new(),
                        diff: None,
                        error: Some(format!(
                            "command '{}' timed out after {}s",
                            cmd, COMMAND_TIMEOUT_SECS
                        )),
                        stop_reason: None,
                    };
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return ToolResult::error(format!("error waiting for process: {}", e)),
        }
    }

    let output = match child.wait_with_output() {
        Ok(o) => o,
        Err(e) => return ToolResult::error(format!("failed to collect output: {}", e)),
    };

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let exit_success = output.status.success();
    let exit_code = output.status.code().unwrap_or(-1);

    let combined = if stderr.is_empty() {
        stdout
    } else if stdout.is_empty() {
        stderr
    } else {
        format!("{}\n--- stderr ---\n{}", stdout, stderr)
    };

    let truncated = truncate_output(&combined);

    if exit_success {
        ToolResult {
            success: true,
            output: truncated,
            diff: None,
            error: None,
            stop_reason: None,
        }
    } else {
        ToolResult {
            success: false,
            output: truncated,
            diff: None,
            error: Some(format!("exited with code {}", exit_code)),
            stop_reason: None,
        }
    }
}

fn is_allowed(program: &str) -> bool {
    let base = std::path::Path::new(program)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(program);

    ALLOWED_COMMANDS.contains(&base)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_successful_command() {
        let dir = tempdir().unwrap();
        let result = execute("ls", dir.path());
        assert!(result.success);
        assert!(result.diff.is_none());
    }

    #[test]
    fn test_failed_command_not_fatal() {
        let dir = tempdir().unwrap();
        let result = execute("python3 -c \"import sys; sys.exit(1)\"", dir.path());
        assert!(!result.success);
        assert!(result.stop_reason.is_none());
    }

    #[test]
    fn test_blocked_command() {
        let dir = tempdir().unwrap();
        let result = execute("rm -rf .", dir.path());
        assert!(!result.success);
        assert!(result.error.unwrap().contains("not allowed"));
        assert!(result.stop_reason.is_none());
    }

    #[test]
    fn test_empty_command() {
        let dir = tempdir().unwrap();
        let result = execute("", dir.path());
        assert!(!result.success);
        assert!(result.stop_reason.is_none());
    }

    #[test]
    fn test_working_directory_is_workspace() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("test.txt"), "hello").unwrap();
        let result = execute("ls", dir.path());
        assert!(result.success);
        assert!(result.output.contains("test.txt"));
    }
}
