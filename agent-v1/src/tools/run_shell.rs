use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use crate::tools::run_command::{self, DEFAULT_TIMEOUT_SECS, MAX_TIMEOUT_SECS};
use crate::tools::{truncate_output, ToolResult};

pub struct RunShellOpts<'a> {
    pub command: &'a str,
    pub cwd: &'a Path,
    pub timeout_secs: u64,
}

/// Run an arbitrary shell string via `sh -c` under the workspace cwd.
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
                        output: run_command::format_structured(
                            -1,
                            duration_ms,
                            &cwd_rel,
                            timeout_secs,
                            "",
                            &format!("timed out after {timeout_secs}s"),
                        ),
                        diff: None,
                        error: Some(format!(
                            "command timed out after {timeout_secs}s"
                        )),
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

    let structured = run_command::format_structured(
        exit_code,
        duration_ms,
        &cwd_rel,
        timeout_secs,
        &stdout,
        &stderr,
    );
    let truncated = truncate_output(&structured);

    ToolResult {
        success: ok,
        output: truncated,
        diff: None,
        error: if ok {
            None
        } else {
            Some(format!("exited with code {exit_code}"))
        },
        stop_reason: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
