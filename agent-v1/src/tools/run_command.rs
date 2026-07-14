use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use crate::tools::{truncate_output, ToolResult};

pub const DEFAULT_TIMEOUT_SECS: u64 = 30;
pub const MAX_TIMEOUT_SECS: u64 = 900;

const ALLOWED_COMMANDS: &[&str] = &[
    "python", "python3", "pytest", "cargo", "npm", "npx", "pnpm", "yarn", "node", "cat", "ls",
    "make", "jest",
];

pub struct RunCommandOpts<'a> {
    pub cmd: &'a str,
    pub cwd: &'a Path,
    pub timeout_secs: u64,
}

pub fn execute(opts: RunCommandOpts<'_>, workspace_root: &Path) -> ToolResult {
    if !workspace_root.exists() {
        return ToolResult::error(format!(
            "workspace_root does not exist: {}",
            workspace_root.display()
        ));
    }

    let parts: Vec<&str> = opts.cmd.split_whitespace().collect();
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

    let timeout_secs = opts.timeout_secs.clamp(1, MAX_TIMEOUT_SECS);
    let cwd_rel = if opts.cwd.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        opts.cwd.to_path_buf()
    };
    let cwd_abs = workspace_root.join(&cwd_rel);

    if !cwd_abs.exists() {
        return ToolResult::error(format!(
            "working_directory not found: {} (under workspace {})",
            cwd_rel.display(),
            workspace_root.display()
        ));
    }
    if !cwd_abs.is_dir() {
        return ToolResult::error(format!(
            "working_directory is not a directory: {}",
            cwd_rel.display()
        ));
    }

    let start = Instant::now();
    let output = Command::new(program)
        .args(args)
        .current_dir(&cwd_abs)
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

    let timeout = Duration::from_secs(timeout_secs);

    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let duration_ms = start.elapsed().as_millis();
                    return ToolResult {
                        success: false,
                        output: format_structured(
                            -1,
                            duration_ms,
                            &cwd_rel,
                            timeout_secs,
                            "",
                            &format!("timed out after {timeout_secs}s"),
                        ),
                        diff: None,
                        error: Some(format!(
                            "command '{}' timed out after {}s",
                            opts.cmd, timeout_secs
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

    let duration_ms = start.elapsed().as_millis();
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let exit_code = output.status.code().unwrap_or(-1);
    let exit_success = output.status.success();

    let structured = format_structured(
        exit_code,
        duration_ms,
        &cwd_rel,
        timeout_secs,
        &stdout,
        &stderr,
    );
    let truncated = truncate_output(&structured);

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

pub fn format_structured(
    exit_code: i32,
    duration_ms: u128,
    cwd: &Path,
    timeout_secs: u64,
    stdout: &str,
    stderr: &str,
) -> String {
    let mut out = format!(
        "exit_code: {exit_code}\nduration_ms: {duration_ms}\ncwd: {}\ntimeout_secs: {timeout_secs}\n",
        cwd.display()
    );
    out.push_str("\n--- stdout ---\n");
    if stdout.is_empty() {
        out.push_str("(empty)\n");
    } else {
        out.push_str(stdout);
        if !stdout.ends_with('\n') {
            out.push('\n');
        }
    }
    out.push_str("\n--- stderr ---\n");
    if stderr.is_empty() {
        out.push_str("(empty)\n");
    } else {
        out.push_str(stderr);
        if !stderr.ends_with('\n') {
            out.push('\n');
        }
    }
    out
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

    fn run(cmd: &str, workspace: &Path) -> ToolResult {
        execute(
            RunCommandOpts {
                cmd,
                cwd: Path::new("."),
                timeout_secs: DEFAULT_TIMEOUT_SECS,
            },
            workspace,
        )
    }

    #[test]
    fn test_successful_command() {
        let dir = tempdir().unwrap();
        let result = run("ls", dir.path());
        assert!(result.success);
        assert!(result.output.contains("exit_code: 0"));
        assert!(result.output.contains("duration_ms:"));
        assert!(result.diff.is_none());
    }

    #[test]
    fn test_failed_command_not_fatal() {
        let dir = tempdir().unwrap();
        let result = run("python3 -c \"import sys; sys.exit(1)\"", dir.path());
        assert!(!result.success);
        assert!(result.output.contains("exit_code: 1"));
        assert!(result.stop_reason.is_none());
    }

    #[test]
    fn test_blocked_command() {
        let dir = tempdir().unwrap();
        let result = run("rm -rf .", dir.path());
        assert!(!result.success);
        assert!(result.error.unwrap().contains("not allowed"));
        assert!(result.stop_reason.is_none());
    }

    #[test]
    fn test_empty_command() {
        let dir = tempdir().unwrap();
        let result = run("", dir.path());
        assert!(!result.success);
        assert!(result.stop_reason.is_none());
    }

    #[test]
    fn test_working_directory_is_workspace() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("test.txt"), "hello").unwrap();
        let result = run("ls", dir.path());
        assert!(result.success);
        assert!(result.output.contains("test.txt"));
    }

    #[test]
    fn test_cwd_subdirectory() {
        let dir = tempdir().unwrap();
        let sub = dir.path().join("apps").join("api");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("only-here.txt"), "x").unwrap();

        let result = execute(
            RunCommandOpts {
                cmd: "ls",
                cwd: Path::new("apps/api"),
                timeout_secs: 10,
            },
            dir.path(),
        );
        assert!(result.success, "{:?}", result.error);
        assert!(result.output.contains("cwd: apps/api"));
        assert!(result.output.contains("only-here.txt"));
    }

    #[test]
    fn test_missing_cwd_errors() {
        let dir = tempdir().unwrap();
        let result = execute(
            RunCommandOpts {
                cmd: "ls",
                cwd: Path::new("nope"),
                timeout_secs: 10,
            },
            dir.path(),
        );
        assert!(!result.success);
    }
}
