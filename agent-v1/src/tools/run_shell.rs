use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use crate::tools::run_command::{self, DEFAULT_TIMEOUT_SECS, MAX_TIMEOUT_SECS};
use crate::tools::{resolve_workspace_cwd, truncate_output, ToolResult};

pub struct RunShellOpts<'a> {
    pub command: &'a str,
    pub cwd: &'a Path,
    pub timeout_secs: u64,
}

/// Run an arbitrary shell string via `sh -c` under a cwd inside the workspace.
///
/// Safety: relative cwd only, no `..`, canonicalize + stay under workspace_root,
/// timeout enforced, output truncated. Exact command is included in the result
/// (and the Action is written to JSONL). Benchmark worktrees stay disposable
/// because cwd cannot escape the task workspace.
pub fn execute(opts: RunShellOpts<'_>, workspace_root: &Path) -> ToolResult {
    if !workspace_root.exists() {
        return ToolResult::error(format!(
            "workspace_root does not exist: {}",
            workspace_root.display()
        ));
    }

    let command = opts.command.trim();
    if command.is_empty() {
        return ToolResult::error("run_shell command must not be empty");
    }

    let timeout_secs = opts.timeout_secs.clamp(1, MAX_TIMEOUT_SECS);
    let (cwd_abs, cwd_rel) = match resolve_workspace_cwd(workspace_root, opts.cwd) {
        Ok(pair) => pair,
        Err(message) => return ToolResult::error(message),
    };

    let start = Instant::now();
    let spawned = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(&cwd_abs)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", std::env::var("HOME").unwrap_or_default())
        .spawn();

    let mut child = match spawned {
        Ok(c) => c,
        Err(e) => {
            return ToolResult::error(format!(
                "failed to spawn sh: {e}. Is a POSIX shell available?"
            ));
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
                        output: format_shell_output(
                            command,
                            -1,
                            duration_ms,
                            &cwd_rel,
                            timeout_secs,
                            "",
                            &format!("timed out after {timeout_secs}s"),
                        ),
                        diff: None,
                        error: Some(format!("command timed out after {timeout_secs}s")),
                        stop_reason: None,
                    };
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return ToolResult::error(format!("error waiting for process: {e}")),
        }
    }

    let output = match child.wait_with_output() {
        Ok(o) => o,
        Err(e) => return ToolResult::error(format!("failed to collect output: {e}")),
    };

    let duration_ms = start.elapsed().as_millis();
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let exit_code = output.status.code().unwrap_or(-1);
    let ok = output.status.success();

    ToolResult {
        success: ok,
        output: format_shell_output(
            command,
            exit_code,
            duration_ms,
            &cwd_rel,
            timeout_secs,
            &stdout,
            &stderr,
        ),
        diff: None,
        error: if ok {
            None
        } else {
            Some(format!("exited with code {exit_code}"))
        },
        stop_reason: None,
    }
}

fn format_shell_output(
    command: &str,
    exit_code: i32,
    duration_ms: u128,
    cwd: &Path,
    timeout_secs: u64,
    stdout: &str,
    stderr: &str,
) -> String {
    let body = run_command::format_structured(
        exit_code,
        duration_ms,
        cwd,
        timeout_secs,
        stdout,
        stderr,
    );
    truncate_output(&format!("command: {command}\n{body}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use tempfile::tempdir;

    #[test]
    fn supports_shell_operators() {
        let dir = tempdir().unwrap();
        let result = execute(
            RunShellOpts {
                command: "echo a && echo b",
                cwd: Path::new("."),
                timeout_secs: DEFAULT_TIMEOUT_SECS,
            },
            dir.path(),
        );
        assert!(result.success, "{:?}", result.error);
        assert!(result.output.contains("command: echo a && echo b"));
        assert!(result.output.contains("exit_code: 0"));
        assert!(result.output.contains('a'));
        assert!(result.output.contains('b'));
    }

    #[test]
    fn non_zero_is_observation() {
        let dir = tempdir().unwrap();
        let result = execute(
            RunShellOpts {
                command: "exit 7",
                cwd: Path::new("."),
                timeout_secs: 10,
            },
            dir.path(),
        );
        assert!(!result.success);
        assert!(result.stop_reason.is_none());
        assert!(result.output.contains("exit_code: 7"));
    }

    #[test]
    fn rejects_parent_dir_escape() {
        let dir = tempdir().unwrap();
        let result = execute(
            RunShellOpts {
                command: "echo hi",
                cwd: Path::new("../"),
                timeout_secs: 10,
            },
            dir.path(),
        );
        assert!(!result.success);
        assert!(
            result
                .error
                .as_deref()
                .unwrap_or("")
                .contains("must not contain '..'"),
            "{:?}",
            result.error
        );
    }

    #[test]
    fn rejects_absolute_cwd() {
        let dir = tempdir().unwrap();
        let result = execute(
            RunShellOpts {
                command: "echo hi",
                cwd: Path::new("/tmp"),
                timeout_secs: 10,
            },
            dir.path(),
        );
        assert!(!result.success);
        assert!(
            result
                .error
                .as_deref()
                .unwrap_or("")
                .contains("must be relative"),
            "{:?}",
            result.error
        );
    }

    #[test]
    fn rejects_symlink_outside_workspace() {
        let dir = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let link = dir.path().join("escape");
        symlink(outside.path(), &link).unwrap();

        let result = execute(
            RunShellOpts {
                command: "echo hi",
                cwd: Path::new("escape"),
                timeout_secs: 10,
            },
            dir.path(),
        );
        assert!(!result.success);
        assert!(
            result
                .error
                .as_deref()
                .unwrap_or("")
                .contains("escape attempt"),
            "{:?}",
            result.error
        );
    }

    #[test]
    fn timeout_still_applies() {
        let dir = tempdir().unwrap();
        let result = execute(
            RunShellOpts {
                command: "sleep 5",
                cwd: Path::new("."),
                timeout_secs: 1,
            },
            dir.path(),
        );
        assert!(!result.success);
        assert!(
            result
                .error
                .as_deref()
                .unwrap_or("")
                .contains("timed out"),
            "{:?}",
            result.error
        );
        assert!(result.output.contains("command: sleep 5"));
    }

    #[test]
    fn large_output_is_bounded() {
        let dir = tempdir().unwrap();
        let result = execute(
            RunShellOpts {
                command: "python3 -c \"print('x'*5000)\"",
                cwd: Path::new("."),
                timeout_secs: 30,
            },
            dir.path(),
        );
        assert!(result.success, "{:?}", result.error);
        assert!(result.output.contains("truncated"), "{}", result.output);
        assert!(result.output.len() < 4000);
    }
}
