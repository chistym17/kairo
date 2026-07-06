use std::path::Path;

use mpatch::{patch_content_str, ApplyOptions};

use crate::tools::{diff, truncate_output, ToolResult};

pub fn execute(path: &Path, patch: &str, workspace_root: &Path) -> ToolResult {
    let canonical_root = match workspace_root.canonicalize() {
        Ok(p) => p,
        Err(e) => return ToolResult::error(format!("workspace_root could not be resolved: {}", e)),
    };

    let full_path = workspace_root.join(path);

    if !full_path.exists() {
        return ToolResult {
            success: false,
            output: String::new(),
            diff: None,
            error: Some(format!(
                "file not found: {} — use write_file to create new files",
                path.display()
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

    let old_content = match std::fs::read_to_string(&canonical_path) {
        Ok(c) => c,
        Err(e) => return ToolResult::error(format!("failed to read {}: {}", path.display(), e)),
    };

    let new_content = match patch_content_str(patch, Some(&old_content), &ApplyOptions::new()) {
        Ok(c) => c,
        Err(e) => {
            return ToolResult {
                success: false,
                output: String::new(),
                diff: None,
                error: Some(format!("patch did not apply: {}", e)),
                stop_reason: None,
            };
        }
    };

    let file_diff = diff::generate(&old_content, &new_content);

    match std::fs::write(&canonical_path, &new_content) {
        Ok(_) => ToolResult {
            success: true,
            output: truncate_output(&format!("applied patch to {}", path.display())),
            diff: Some(file_diff),
            error: None,
            stop_reason: None,
        },
        Err(e) => ToolResult::error(format!("failed to write {}: {}", path.display(), e)),
    }
}


