use std::path::{Path, PathBuf};

use crate::tools::git_status::run_git;
use crate::tools::ToolResult;

/// Context from the current run for clearer diff messaging.
#[derive(Debug, Clone, Default)]
pub struct GitDiffContext {
    pub files_ever_modified: Vec<String>,
}

/// Show unstaged+untracked-aware working tree diff (`git diff` + optional path).
/// Uses `git diff HEAD` when a commit exists so new files after add aren't missed;
/// falls back to `git diff` for brand-new repos with no commits.
pub fn execute(
    path: &Option<PathBuf>,
    workspace_root: &Path,
    ctx: Option<&GitDiffContext>,
) -> ToolResult {
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
            return annotate_run_context(format_clean_diff(run_git(workspace_root, &fb), path, ctx), path, ctx);
        }
    }

    annotate_run_context(format_clean_diff(result, path, ctx), path, ctx)
}

fn annotate_run_context(
    result: ToolResult,
    path: &Option<PathBuf>,
    ctx: Option<&GitDiffContext>,
) -> ToolResult {
    if !result.success {
        return result;
    }
    let Some(ctx) = ctx else {
        return result;
    };
    if ctx.files_ever_modified.is_empty() {
        return result;
    }

    let path_key = path
        .as_ref()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .filter(|p| !p.is_empty() && p != ".");

    let mut header = vec![
        "## Run-relative workspace state".to_string(),
        format!(
            "Files modified this run: {}",
            ctx.files_ever_modified.join(", ")
        ),
    ];

    let body = result.output.trim();
    let is_clean = body.is_empty()
        || body == "(no diff)"
        || body == "(clean — no changes)"
        || body.contains("clean relative to HEAD");

    if let Some(pk) = &path_key {
        let touched = ctx.files_ever_modified.iter().any(|f| f == pk || f.ends_with(pk.as_str()));
        if touched && is_clean {
            header.push(format!(
                "This run edited {pk}; current content matches HEAD \
                 (your change restored the committed version, or was reverted)."
            ));
        } else if touched && !is_clean {
            header.push(format!(
                "This run edited {pk}. Diff below is versus HEAD and may include \
                 pre-existing uncommitted changes unrelated to your latest edit. \
                 Hunks that match HEAD are no longer shown."
            ));
        }
    } else if !is_clean {
        header.push(
            "Diff below is versus HEAD. Files you edited this run may have restored \
             some lines to HEAD; remaining hunks can be pre-existing uncommitted work."
                .to_string(),
        );
    }

    header.push(String::new());
    header.push(result.output);

    ToolResult {
        success: true,
        output: header.join("\n"),
        diff: None,
        error: None,
        stop_reason: None,
    }
}

fn format_clean_diff(
    result: ToolResult,
    path: &Option<PathBuf>,
    ctx: Option<&GitDiffContext>,
) -> ToolResult {
    if !result.success {
        return result;
    }
    let trimmed = result.output.trim();
    if !trimmed.is_empty()
        && trimmed != "(no diff)"
        && trimmed != "(clean — no changes)"
    {
        return result;
    }

    let path_hint = path
        .as_ref()
        .map(|p| p.to_string_lossy().to_string())
        .filter(|p| !p.is_empty() && p != ".")
        .unwrap_or_else(|| "repository".to_string());

    let mut lines = vec![
        format!("Working tree is clean relative to HEAD for {path_hint}."),
        "Current file content matches the committed version.".to_string(),
        "No uncommitted changes remain.".to_string(),
    ];

    if let Some(ctx) = ctx {
        if !ctx.files_ever_modified.is_empty() {
            lines.push(format!(
                "This run modified: {}. Those edits now match HEAD (or were reverted).",
                ctx.files_ever_modified.join(", ")
            ));
        }
    }

    ToolResult {
        success: true,
        output: lines.join("\n"),
        diff: None,
        error: None,
        stop_reason: None,
    }
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

        let result = execute(&None, dir.path(), None);
        assert!(result.success, "{:?}", result.error);
        assert!(result.output.contains("two") || result.output.contains("a.txt"), "{}", result.output);
    }

    #[test]
    fn clean_diff_explains_head_match() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());

        let result = execute(&None, dir.path(), Some(&GitDiffContext {
            files_ever_modified: vec!["a.txt".into()],
        }));
        assert!(result.success);
        assert!(result.output.contains("clean relative to HEAD"));
        assert!(result.output.contains("This run modified"));
        assert!(result.output.contains("Run-relative workspace state"));
    }

    #[test]
    fn dirty_diff_notes_preexisting_when_run_edited() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        // Pre-existing uncommitted change.
        fs::write(dir.path().join("a.txt"), "preexisting\n").unwrap();

        let result = execute(
            &Some(PathBuf::from("a.txt")),
            dir.path(),
            Some(&GitDiffContext {
                files_ever_modified: vec!["a.txt".into()],
            }),
        );
        assert!(result.success);
        assert!(result.output.contains("Run-relative workspace state"));
        assert!(result.output.contains("pre-existing"));
        assert!(result.output.contains("preexisting"));
    }
}
