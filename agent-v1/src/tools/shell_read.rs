use std::path::{Path, PathBuf};

/// Detect simple shell commands that are really file reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellFileReadIntent {
    pub path: PathBuf,
    pub offset: u32,
    pub limit: u32,
}

/// Returns a file-read intent for obvious `cat` / `head` / `tail` / `sed -n` patterns.
pub fn detect_shell_file_read(command: &str) -> Option<ShellFileReadIntent> {
    let trimmed = command.trim();
    let parts: Vec<&str> = trimmed.split_whitespace().collect();
    if parts.is_empty() {
        return None;
    }

    match parts[0] {
        "cat" => {
            let path = parts.get(1)?;
            Some(ShellFileReadIntent {
                path: PathBuf::from(path),
                offset: 1,
                limit: 250,
            })
        }
        "head" => {
            let mut limit = 250u32;
            let mut path_idx = 1usize;
            if parts.len() >= 3 && parts[1] == "-n" {
                limit = parts[2].parse().unwrap_or(250);
                path_idx = 3;
            }
            let path = parts.get(path_idx)?;
            Some(ShellFileReadIntent {
                path: PathBuf::from(path),
                offset: 1,
                limit,
            })
        }
        "tail" => {
            let mut limit = 250u32;
            let mut path_idx = 1usize;
            if parts.len() >= 3 && parts[1] == "-n" {
                limit = parts[2].parse().unwrap_or(250);
                path_idx = 3;
            }
            let path = parts.get(path_idx)?;
            Some(ShellFileReadIntent {
                path: PathBuf::from(path),
                offset: 1,
                limit,
            })
        }
        "sed" => parse_sed_read(trimmed),
        "python3" | "python" => {
            if trimmed.contains("open(") {
                // Best-effort: open('path') — record path only.
                if let Some(start) = trimmed.find("open(") {
                    let rest = &trimmed[start + 5..];
                    let quote = rest.chars().next()?;
                    if quote != '\'' && quote != '"' {
                        return None;
                    }
                    let end = rest[1..].find(quote)?;
                    let path = &rest[1..end + 1];
                    return Some(ShellFileReadIntent {
                        path: PathBuf::from(path),
                        offset: 1,
                        limit: 250,
                    });
                }
            }
            None
        }
        _ => None,
    }
}

fn parse_sed_read(command: &str) -> Option<ShellFileReadIntent> {
    // sed -n '10,20p' file
    if !command.contains("-n") {
        return None;
    }
    let parts: Vec<&str> = command.split_whitespace().collect();
    let pattern = parts.iter().find(|p| p.contains('p'))?;
    let nums = pattern.trim_matches(|c| c == '\'' || c == '"').trim_end_matches('p');
    let (start, end) = if let Some((a, b)) = nums.split_once(',') {
        (a.parse().ok()?, b.parse().ok()?)
    } else {
        let n: u32 = nums.parse().ok()?;
        (n, n)
    };
    let path = parts.last()?;
    if path.contains('p') || path.starts_with('-') {
        return None;
    }
    Some(ShellFileReadIntent {
        path: PathBuf::from(path),
        offset: start,
        limit: end.saturating_sub(start).saturating_add(1),
    })
}

pub fn apply_read_governance_to_shell(
    read_cache: &mut crate::tools::read_cache::ReadCache,
    workspace_root: &Path,
    intent: &ShellFileReadIntent,
) -> Option<crate::tools::ToolResult> {
    use crate::tools::read_cache::ReadCacheOutcome;
    match read_cache.before_read(
        workspace_root,
        &intent.path,
        intent.offset,
        intent.limit,
    ) {
        ReadCacheOutcome::Miss => None,
        ReadCacheOutcome::Hit => read_cache.try_hit(
            workspace_root,
            &intent.path,
            intent.offset,
            intent.limit,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_cat() {
        let i = detect_shell_file_read("cat frontend/src/lib/api.ts").unwrap();
        assert_eq!(i.path, PathBuf::from("frontend/src/lib/api.ts"));
    }

    #[test]
    fn detects_head_n() {
        let i = detect_shell_file_read("head -n 50 backend/views.py").unwrap();
        assert_eq!(i.limit, 50);
    }
}
