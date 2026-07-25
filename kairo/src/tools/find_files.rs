use std::fs;
use std::path::{Path, PathBuf};

use crate::tools::ignore::is_ignored_dir_name;
use crate::tools::{truncate_output, ToolResult};

const DEFAULT_MAX_ENTRIES: usize = 200;

/// Find files whose relative path matches a simple glob `pattern`.
/// Supports `*` (within a segment), `**` (across segments), and `?`.
pub fn execute(pattern: &str, path: &PathBuf, workspace_root: &Path) -> ToolResult {
    if !workspace_root.exists() {
        return ToolResult::error(format!(
            "workspace_root does not exist: {}",
            workspace_root.display()
        ));
    }

    let pat = pattern.trim();
    if pat.is_empty() {
        return ToolResult::error("find_files pattern must not be empty");
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

    let mut hits: Vec<String> = Vec::new();
    let mut truncated = false;

    if let Err(e) = walk_find(workspace_root, &root, pat, &mut hits, &mut truncated) {
        return ToolResult::error(format!("find_files failed: {e}"));
    }

    hits.sort();
    let header = format!(
        "find_files pattern={:?} under '{}' — {} file(s){}",
        pat,
        rel.display(),
        hits.len(),
        if truncated {
            format!(" (stopped at {DEFAULT_MAX_ENTRIES})")
        } else {
            String::new()
        }
    );
    let body = if hits.is_empty() {
        "(no matches)".to_string()
    } else {
        hits.join("\n")
    };

    ToolResult {
        success: true,
        output: truncate_output(&format!("{header}\n{body}")),
        diff: None,
        error: None,
        stop_reason: None,
    }
}

fn walk_find(
    workspace_root: &Path,
    cur: &Path,
    pattern: &str,
    hits: &mut Vec<String>,
    truncated: &mut bool,
) -> std::io::Result<()> {
    if hits.len() >= DEFAULT_MAX_ENTRIES {
        *truncated = true;
        return Ok(());
    }

    let md = fs::metadata(cur)?;
    if md.is_file() {
        maybe_push(workspace_root, cur, pattern, hits);
        return Ok(());
    }

    for entry in fs::read_dir(cur)? {
        if hits.len() >= DEFAULT_MAX_ENTRIES {
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
            walk_find(workspace_root, &p, pattern, hits, truncated)?;
        } else {
            maybe_push(workspace_root, &p, pattern, hits);
            if hits.len() >= DEFAULT_MAX_ENTRIES {
                *truncated = true;
                break;
            }
        }
    }
    Ok(())
}

fn maybe_push(workspace_root: &Path, path: &Path, pattern: &str, hits: &mut Vec<String>) {
    let rel = path
        .strip_prefix(workspace_root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");

    // Patterns without `/` also match basename (e.g. `*.tsx`).
    let ok = glob_match(&rel, pattern)
        || (!pattern.contains('/') && glob_match(file_name, pattern));
    if ok {
        hits.push(rel);
    }
}

/// Minimal glob: `*` = any chars except `/`, `**` = any chars including `/`, `?` = one non-`/` char.
fn glob_match(text: &str, pattern: &str) -> bool {
    glob_match_bytes(text.as_bytes(), pattern.as_bytes())
}

fn glob_match_bytes(text: &[u8], pattern: &[u8]) -> bool {
    if pattern.is_empty() {
        return text.is_empty();
    }

    if pattern[0] == b'*' {
        if pattern.len() >= 2 && pattern[1] == b'*' {
            let rest = if pattern.len() >= 3 && pattern[2] == b'/' {
                &pattern[3..]
            } else {
                &pattern[2..]
            };
            if rest.is_empty() {
                return true;
            }
            for i in 0..=text.len() {
                if glob_match_bytes(&text[i..], rest) {
                    return true;
                }
            }
            return false;
        }

        let rest = &pattern[1..];
        if rest.is_empty() {
            return !text.contains(&b'/');
        }
        for i in 0..=text.len() {
            if text[..i].contains(&b'/') {
                break;
            }
            if glob_match_bytes(&text[i..], rest) {
                return true;
            }
        }
        return false;
    }

    if pattern[0] == b'?' {
        if text.is_empty() || text[0] == b'/' {
            return false;
        }
        return glob_match_bytes(&text[1..], &pattern[1..]);
    }

    if text.first() == Some(&pattern[0]) {
        return glob_match_bytes(&text[1..], &pattern[1..]);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn finds_by_extension() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/a.tsx"), "a").unwrap();
        fs::write(dir.path().join("src/b.ts"), "b").unwrap();
        fs::write(dir.path().join("readme.md"), "r").unwrap();

        let result = execute("*.tsx", &PathBuf::from("."), dir.path());
        assert!(result.success, "{:?}", result.error);
        assert!(result.output.contains("src/a.tsx"));
        assert!(!result.output.contains("src/b.ts"));
    }

    #[test]
    fn glob_star_basics() {
        assert!(glob_match("a.tsx", "*.tsx"));
        assert!(!glob_match("a.ts", "*.tsx"));
        assert!(glob_match("src/components/Nav.tsx", "**/*.tsx"));
        assert!(!glob_match("src/a/b.ts", "src/*.ts"));
        assert!(glob_match("src/a.ts", "src/*.ts"));
    }
}
