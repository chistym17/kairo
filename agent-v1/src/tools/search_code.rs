use std::fs;
use std::path::{Path, PathBuf};

use crate::tools::ignore::is_ignored_dir_name;
use crate::tools::{truncate_output, ToolResult};

const DEFAULT_MAX_MATCHES: usize = 40;
const MAX_FILE_BYTES: u64 = 512 * 1024;
const MAX_LINE_CHARS: usize = 200;

/// Search file contents for `query` under `path` (relative to workspace).
pub fn execute(query: &str, path: &PathBuf, workspace_root: &Path) -> ToolResult {
    if !workspace_root.exists() {
        return ToolResult::error(format!(
            "workspace_root does not exist: {}",
            workspace_root.display()
        ));
    }

    let q = query.trim();
    if q.is_empty() {
        return ToolResult::error("search_code query must not be empty");
    }

    let rel = if path.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        path.clone()
    };
    let root = workspace_root.join(&rel);

    if !root.exists() {
        return ToolResult::error(format!(
            "path not found: {} (resolved from workspace: {})",
            rel.display(),
            workspace_root.display()
        ));
    }

    let needle = q.to_ascii_lowercase();
    let mut matches: Vec<String> = Vec::new();
    let mut files_scanned: usize = 0;
    let mut truncated = false;

    if let Err(e) = walk_search(
        workspace_root,
        &root,
        &needle,
        &mut matches,
        &mut files_scanned,
        &mut truncated,
    ) {
        return ToolResult::error(format!("search_code failed: {e}"));
    }

    let header = format!(
        "search_code query={:?} under '{}' — {} match(es), {} file(s) scanned{}",
        q,
        rel.display(),
        matches.len(),
        files_scanned,
        if truncated {
            format!(" (stopped at {DEFAULT_MAX_MATCHES} matches)")
        } else {
            String::new()
        }
    );

    let body = if matches.is_empty() {
        "(no matches)".to_string()
    } else {
        matches.join("\n")
    };

    ToolResult {
        success: true,
        output: truncate_output(&format!("{header}\n{body}")),
        diff: None,
        error: None,
        stop_reason: None,
    }
}

fn walk_search(
    workspace_root: &Path,
    cur: &Path,
    needle: &str,
    matches: &mut Vec<String>,
    files_scanned: &mut usize,
    truncated: &mut bool,
) -> std::io::Result<()> {
    if matches.len() >= DEFAULT_MAX_MATCHES {
        *truncated = true;
        return Ok(());
    }

    let md = fs::metadata(cur)?;
    if md.is_file() {
        *files_scanned += 1;
        search_file(workspace_root, cur, needle, matches, truncated)?;
        return Ok(());
    }

    for entry in fs::read_dir(cur)? {
        if matches.len() >= DEFAULT_MAX_MATCHES {
            *truncated = true;
            break;
        }
        let entry = entry?;
        let p = entry.path();
        if p.is_dir() {
            if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                if is_ignored_dir_name(name) {
                    continue;
                }
            }
            walk_search(workspace_root, &p, needle, matches, files_scanned, truncated)?;
        } else {
            *files_scanned += 1;
            search_file(workspace_root, &p, needle, matches, truncated)?;
        }
    }
    Ok(())
}

fn search_file(
    workspace_root: &Path,
    path: &Path,
    needle: &str,
    matches: &mut Vec<String>,
    truncated: &mut bool,
) -> std::io::Result<()> {
    if matches.len() >= DEFAULT_MAX_MATCHES {
        *truncated = true;
        return Ok(());
    }

    let meta = fs::metadata(path)?;
    if meta.len() > MAX_FILE_BYTES {
        return Ok(());
    }

    let bytes = fs::read(path)?;
    if bytes.contains(&0) {
        return Ok(()); // skip binary
    }
    let Ok(text) = String::from_utf8(bytes) else {
        return Ok(());
    };

    let rel = path
        .strip_prefix(workspace_root)
        .unwrap_or(path)
        .to_string_lossy();

    for (idx, line) in text.lines().enumerate() {
        if matches.len() >= DEFAULT_MAX_MATCHES {
            *truncated = true;
            break;
        }
        if line.to_ascii_lowercase().contains(needle) {
            let mut snippet = line.trim().to_string();
            if snippet.len() > MAX_LINE_CHARS {
                snippet.truncate(MAX_LINE_CHARS);
                snippet.push('…');
            }
            matches.push(format!("{rel}:{}: {snippet}", idx + 1));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn finds_case_insensitive_matches() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(
            dir.path().join("src/Navbar.tsx"),
            "export function Navbar() {\n  return null;\n}\n",
        )
        .unwrap();
        fs::write(dir.path().join("src/other.ts"), "const x = 1;\n").unwrap();

        let result = execute(
            "navbar",
            &PathBuf::from("."),
            dir.path(),
        );
        assert!(result.success, "{:?}", result.error);
        assert!(result.output.contains("Navbar.tsx"));
        assert!(result.output.contains("export function Navbar"));
    }

    #[test]
    fn empty_query_errors() {
        let dir = tempfile::tempdir().unwrap();
        let result = execute("  ", &PathBuf::from("."), dir.path());
        assert!(!result.success);
    }
}
