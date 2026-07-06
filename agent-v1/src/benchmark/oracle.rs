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
