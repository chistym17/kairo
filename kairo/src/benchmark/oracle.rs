use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};

#[derive(Debug, Clone)]
pub struct OracleResult {
    pub passed: bool,
    pub exit_code: i32,
    pub output: String,
}

/// Run the task verification command in the task workspace.
pub fn run_oracle(workspace: &Path, verify_cmd: &str) -> Result<OracleResult> {
    let output = Command::new("sh")
        .arg("-c")
        .arg(verify_cmd)
        .current_dir(workspace)
        .output()
        .with_context(|| {
            format!(
                "failed to run verify command in {}",
                workspace.display()
            )
        })?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}{stderr}");

    Ok(OracleResult {
        passed: output.status.success(),
        exit_code: output.status.code().unwrap_or(-1),
        output: combined,
    })
}

/// Strict oracle: run tests and fail the run if any readonly files were modified.
///
/// This closes the \"pytest green by editing tests\" loophole.
pub fn run_oracle_strict(
    workspace: &Path,
    verify_cmd: &str,
    files_readonly: &[String],
) -> Result<(OracleResult, Vec<String>)> {
    let oracle = run_oracle(workspace, verify_cmd)?;
    let violations = readonly_violations(workspace, files_readonly)?;

    if violations.is_empty() {
        return Ok((oracle, violations));
    }

    let mut out = oracle.output.clone();
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("\n[benchmark] readonly file violation(s):\n");
    for v in &violations {
        out.push_str(&format!("- {v}\n"));
    }

    Ok((
        OracleResult {
            passed: false,
            exit_code: oracle.exit_code,
            output: out,
        },
        violations,
    ))
}

fn readonly_violations(workspace: &Path, files_readonly: &[String]) -> Result<Vec<String>> {
    if files_readonly.is_empty() {
        return Ok(vec![]);
    }

    // Workspaces are git worktrees in Phase 4.
    // If git isn't available for some reason, don't silently pass.
    let output = Command::new("git")
        .arg("diff")
        .arg("--name-only")
        .current_dir(workspace)
        .output()
        .with_context(|| format!("failed to run git diff in {}", workspace.display()))?;

    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "git diff failed in {}:\nstdout:\n{}\nstderr:\n{}",
            workspace.display(),
            stdout,
            stderr
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let changed: Vec<String> = stdout
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect();

    let mut violations = Vec::new();
    for ro in files_readonly {
        if changed.iter().any(|c| c == ro) {
            violations.push(ro.clone());
        }
    }

    Ok(violations)
}
