use std::path::Path;

use crate::tools::ToolResult;

/// Allow full medium source files; still cap huge dumps.
const READ_FILE_MAX_CHARS: usize = 50_000;

pub fn execute(path: &Path, workspace_root: &Path) -> ToolResult {
    let full_path = workspace_root.join(path);

    let canonical_root = match workspace_root.canonicalize() {
        Ok(p) => p,
        Err(e) => return ToolResult::error(format!("workspace_root could not be resolved: {}", e)),
    };

    if !full_path.exists() {
        return ToolResult {
            success: false,
            output: String::new(),
            diff: None,
            error: Some(format!(
                "file not found: {} (resolved from workspace: {})",
                path.display(),
                workspace_root.display()
            )),
            stop_reason: None,
        };
    }

    let canonical_path = match full_path.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            return ToolResult::error(format!("could not resolve path {}: {}", path.display(), e))
        }
    };

    if !canonical_path.starts_with(&canonical_root) {
        return ToolResult::error(format!(
            "path escape attempt: {} is outside workspace {}",
            path.display(),
            workspace_root.display()
        ));
    }

    match std::fs::read_to_string(&canonical_path) {
        Ok(contents) => {
            let output = if contents.len() <= READ_FILE_MAX_CHARS {
                contents
            } else {
                format!(
                    "{}\n...[truncated — {} total chars]",
                    &contents[..READ_FILE_MAX_CHARS],
                    contents.len()
                )
            };
            ToolResult {
                success: true,
                output,
                diff: None,
                error: None,
                stop_reason: None,
            }
        }
        Err(e) => ToolResult::error(format!("failed to read {}: {}", path.display(), e)),
    }
}
