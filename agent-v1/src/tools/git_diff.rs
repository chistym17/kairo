use std::path::{Path, PathBuf};

use crate::tools::git_status::run_git;
use crate::tools::ToolResult;

/// Show unstaged+untracked-aware working tree diff (`git diff` + optional path).
/// Uses `git diff HEAD` when a commit exists so new files after add aren't missed;
/// falls back to `git diff` for brand-new repos with no commits.
pub fn execute(path: &Option<PathBuf>, workspace_root: &Path) -> ToolResult {
    if !workspace_root.exists() {
        return ToolResult::error(format!(
            "workspace_root does not exist: {}",
            workspace_root.display()
        ));
    }

    let mut args: Vec<String> = vec!["diff".to_string(), "HEAD".to_string()];
    if let Some(p) = path {
        let rel = p.to_string_lossy();
        if !rel.is_empty() && rel != "." {
            args.push("--".to_string());
            args.push(rel.to_string());
        }
    }

    let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let result = run_git(workspace_root, &args_ref);

    // Brand-new repo (no HEAD): fall back to unstaged diff.
    if !result.success {
        let err = result.error.as_deref().unwrap_or("");
        if err.contains("bad revision")
            || err.contains("unknown revision")
            || err.contains("ambiguous argument 'HEAD'")
            || err.contains("Needed a single revision")
        {
            let mut fallback = vec!["diff".to_string()];
            if let Some(p) = path {
                let rel = p.to_string_lossy();
                if !rel.is_empty() && rel != "." {
                    fallback.push("--".to_string());
                    fallback.push(rel.to_string());
                }
            }
            let fb: Vec<&str> = fallback.iter().map(|s| s.as_str()).collect();
            return run_git(workspace_root, &fb);
        }
    }

    if result.success && result.output.trim().is_empty() {
        return ToolResult {
            success: true,
            output: "(no diff)".to_string(),
            diff: None,
            error: None,
            stop_reason: None,
        };
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;

    fn init_repo(dir: &Path) {
        assert!(Command::new("git")
            .args(["init"])
            .current_dir(dir)
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .unwrap()
            .status
            .success());
        let _ = Command::new("git")
            .args(["config", "user.email", "test@example.com"])
            .current_dir(dir)
            .output();
        let _ = Command::new("git")
            .args(["config", "user.name", "test"])
            .current_dir(dir)
            .output();
        fs::write(dir.join("a.txt"), "one\n").unwrap();
        assert!(Command::new("git")
            .args(["add", "a.txt"])
            .current_dir(dir)
            .output()
            .unwrap()
            .status
            .success());
        assert!(Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(dir)
            .output()
            .unwrap()
            .status
            .success());
    }

    #[test]
    fn shows_working_tree_diff() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        fs::write(dir.path().join("a.txt"), "two\n").unwrap();

        let result = execute(&None, dir.path());
        assert!(result.success, "{:?}", result.error);
        assert!(result.output.contains("two") || result.output.contains("a.txt"), "{}", result.output);
    }
}
