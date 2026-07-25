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

pub fn diff_preview(workspace_dir: &Path, max_lines: usize) -> Result<String> {
    let output = Command::new("git")
        .arg("diff")
        .arg("--no-color")
        .current_dir(workspace_dir)
        .output()
        .with_context(|| format!("failed to run git diff in {}", workspace_dir.display()))?;

    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "git diff failed in {}:\nstdout:\n{}\nstderr:\n{}",
            workspace_dir.display(),
            stdout,
            stderr
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut lines = stdout.lines();
    let mut out = String::new();

    for _ in 0..max_lines {
        if let Some(line) = lines.next() {
            out.push_str(line);
            out.push('\n');
        } else {
            break;
        }
    }

    if lines.next().is_some() {
        out.push_str("... (diff truncated)\n");
    }

    Ok(out.trim_end().to_string())
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
