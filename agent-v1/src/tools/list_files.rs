use std::path::{Path, PathBuf};

use crate::tools::ToolResult;

const DEFAULT_MAX_DEPTH: u32 = 4;
const DEFAULT_MAX_ENTRIES: u32 = 500;

const IGNORED_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    ".next",
    ".turbo",
    ".cache",
];

pub fn execute(
    path: &PathBuf,
    max_depth: u32,
    max_entries: u32,
    workspace_root: &Path,
) -> ToolResult {
    if !workspace_root.exists() {
        return ToolResult::error(format!(
            "workspace_root does not exist: {}",
            workspace_root.display()
        ));
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

    let max_depth = if max_depth == 0 {
        DEFAULT_MAX_DEPTH
    } else {
        max_depth.max(1).min(25)
    };
    let max_entries = if max_entries == 0 {
        DEFAULT_MAX_ENTRIES
    } else {
        max_entries.max(1).min(10_000)
    };

    let mut out: Vec<String> = Vec::new();
    let mut count: u32 = 0;

    fn walk(
        base: &Path,
        cur: &Path,
        depth: u32,
        max_depth: u32,
        max_entries: u32,
        out: &mut Vec<String>,
        count: &mut u32,
    ) -> std::io::Result<()> {
        if *count >= max_entries {
            return Ok(());
        }
        if depth > max_depth {
            return Ok(());
        }

        let md = std::fs::metadata(cur)?;
        if md.is_file() {
            let rel = cur.strip_prefix(base).unwrap_or(cur);
            out.push(rel.to_string_lossy().to_string());
            *count += 1;
            return Ok(());
        }

        let read_dir = std::fs::read_dir(cur)?;
        for entry in read_dir {
            if *count >= max_entries {
                break;
            }
            let entry = entry?;
            let p = entry.path();

            if p.is_dir() {
                if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                    if IGNORED_DIRS.contains(&name) {
                        continue;
                    }
                }
                walk(base, &p, depth + 1, max_depth, max_entries, out, count)?;
            } else {
                let rel = p.strip_prefix(base).unwrap_or(&p);
                out.push(rel.to_string_lossy().to_string());
                *count += 1;
            }
        }

        Ok(())
    }

    let base = &root;
    let res = walk(base, base, 0, max_depth, max_entries, &mut out, &mut count);
    match res {
        Ok(()) => {
            out.sort();
            let header = format!(
                "listed {} entries under '{}' (max_depth={}, max_entries={})",
                out.len(),
                rel.display(),
                max_depth,
                max_entries
            );
            let body = if out.is_empty() {
                "(empty)".to_string()
            } else {
                out.join("\n")
            };
            ToolResult {
                success: true,
                output: format!("{}\n{}", header, body),
                diff: None,
                error: None,
                stop_reason: None,
            }
        }
        Err(e) => ToolResult::error(format!("failed to list files: {}", e)),
    }
}

