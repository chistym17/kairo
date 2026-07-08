use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};

pub fn reset_worktree(repo_dir: &Path, git_ref: &str, workspace_dir: &Path) -> Result<()> {
    let workspace_abs = if workspace_dir.is_absolute() {
        workspace_dir.to_path_buf()
    } else {
        std::env::current_dir()
            .context("failed to get current directory")?
            .join(workspace_dir)
    };

    if let Some(parent) = workspace_abs.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to create worktree parent directory: {}",
                parent.display()
            )
        })?;
    }

    if workspace_abs.exists() {
        let _ = git(
            repo_dir,
            &[
                "worktree",
                "remove",
                "--force",
                workspace_abs.to_string_lossy().as_ref(),
            ],
        );

        if workspace_abs.exists() {
            std::fs::remove_dir_all(&workspace_abs).with_context(|| {
                format!(
                    "failed to delete existing workspace directory: {}",
                    workspace_abs.display()
                )
            })?;
        }
    }

    git(
        repo_dir,
        &[
            "worktree",
            "add",
            "--detach",
            workspace_abs.to_string_lossy().as_ref(),
            git_ref,
        ],
    )
    .with_context(|| {
        format!(
            "failed to create worktree {} at {}",
            workspace_abs.display(),
            git_ref
        )
    })?;

    Ok(())
}

fn git(repo_dir: &Path, args: &[&str]) -> Result<()> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_dir)
        .args(args)
        .output()
        .with_context(|| format!("failed to execute git -C {} {:?}", repo_dir.display(), args))?;

    if output.status.success() {
        return Ok(());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    anyhow::bail!(
        "git command failed: git -C {} {:?}\nstdout:\n{}\nstderr:\n{}",
        repo_dir.display(),
        args,
        stdout,
        stderr
    )
}
