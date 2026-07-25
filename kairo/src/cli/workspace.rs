//! Detect the workspace root and display helpers for the CLI.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Resolve the workspace the agent should operate on.
///
/// Priority:
/// 1. `explicit` (`--workspace`), when provided
/// 2. nearest parent directory containing `.git`
/// 3. current working directory
pub fn detect_workspace(explicit: Option<PathBuf>) -> std::io::Result<PathBuf> {
    if let Some(path) = explicit {
        let path = if path.exists() {
            path.canonicalize()?
        } else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("workspace does not exist: {}", path.display()),
            ));
        };
        if !path.is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("workspace is not a directory: {}", path.display()),
            ));
        }
        return Ok(path);
    }

    let current = std::env::current_dir()?;

    for directory in current.ancestors() {
        if directory.join(".git").exists() {
            return directory.canonicalize();
        }
    }

    current.canonicalize()
}

/// Current branch name, or `None` when not a git repo / unavailable.
pub fn current_branch(workspace: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["-C", &workspace.to_string_lossy(), "rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if name.is_empty() || name == "HEAD" {
        // Detached HEAD — show short SHA if possible.
        let sha = Command::new("git")
            .args(["-C", &workspace.to_string_lossy(), "rev-parse", "--short", "HEAD"])
            .output()
            .ok()?;
        if sha.status.success() {
            let short = String::from_utf8_lossy(&sha.stdout).trim().to_string();
            if !short.is_empty() {
                return Some(format!("detached@{short}"));
            }
        }
        return None;
    }
    Some(name)
}

/// Shorten `$HOME` prefix to `~` for display only.
pub fn display_path(path: &Path) -> String {
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        if let Ok(rel) = path.strip_prefix(&home) {
            if rel.as_os_str().is_empty() {
                return "~".to_string();
            }
            return format!("~/{}", rel.display());
        }
    }
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn explicit_workspace_wins() {
        let dir = tempdir().unwrap();
        let nested = dir.path().join("nested");
        fs::create_dir_all(&nested).unwrap();
        fs::create_dir(dir.path().join(".git")).unwrap();

        let found = detect_workspace(Some(nested.clone())).unwrap();
        assert_eq!(found, nested.canonicalize().unwrap());
    }

    #[test]
    fn finds_nearest_git_root_from_nested_cwd() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("shop");
        let nested = root.join("src/components");
        fs::create_dir_all(&nested).unwrap();
        fs::create_dir(root.join(".git")).unwrap();

        let previous = std::env::current_dir().unwrap();
        std::env::set_current_dir(&nested).unwrap();
        let found = detect_workspace(None).unwrap();
        std::env::set_current_dir(previous).unwrap();

        assert_eq!(found, root.canonicalize().unwrap());
    }

    #[test]
    fn falls_back_to_cwd_without_git() {
        let dir = tempdir().unwrap();
        let previous = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();
        let found = detect_workspace(None).unwrap();
        std::env::set_current_dir(previous).unwrap();

        assert_eq!(found, dir.path().canonicalize().unwrap());
    }

    #[test]
    fn missing_explicit_workspace_errors() {
        let err = detect_workspace(Some(PathBuf::from("/no/such/kairo-workspace"))).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }
}
