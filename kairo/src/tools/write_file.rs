use std::path::Path;

use crate::tools::{diff, truncate_output, ToolResult};

pub fn execute(path: &Path, content: &str, workspace_root: &Path) -> ToolResult {
    let canonical_root = match workspace_root.canonicalize() {
        Ok(p) => p,
        Err(e) => return ToolResult::error(format!("workspace_root could not be resolved: {}", e)),
    };

    let full_path = workspace_root.join(path);

    let parent = match full_path.parent() {
        Some(p) => p,
        None => return ToolResult::error("path has no parent directory"),
    };

    if !parent.exists() {
        return ToolResult {
            success: false,
            output: String::new(),
            diff: None,
            error: Some(format!(
                "parent directory does not exist: {}",
                parent.display()
            )),
            stop_reason: None,
        };
    }

    let canonical_parent = match parent.canonicalize() {
        Ok(p) => p,
        Err(e) => return ToolResult::error(format!("could not resolve parent directory: {}", e)),
    };

    if !canonical_parent.starts_with(&canonical_root) {
        return ToolResult::error(format!(
            "path escape attempt: {} is outside workspace {}",
            path.display(),
            workspace_root.display()
        ));
    }

    let old_content = std::fs::read_to_string(&full_path).unwrap_or_default();

    let file_diff = diff::generate(&old_content, content);

    match std::fs::write(&full_path, content) {
        Ok(_) => ToolResult {
            success: true,
            output: truncate_output(&format!(
                "wrote {} bytes to {}",
                content.len(),
                path.display()
            )),
            diff: Some(file_diff),
            error: None,
            stop_reason: None,
        },
        Err(e) => ToolResult::error(format!("failed to write {}: {}", path.display(), e)),
    }
}
