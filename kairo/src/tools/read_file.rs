use std::path::Path;

use crate::tools::symbol_expand::expand_read_window;
use crate::tools::ToolResult;

pub const DEFAULT_OFFSET: u32 = 1;
pub const DEFAULT_LIMIT: u32 = 250;
pub const MAX_LIMIT: u32 = 500;

/// Soft char cap within a line window (avoid huge single-line dumps).
const MAX_WINDOW_CHARS: usize = 50_000;

pub fn execute(
    path: &Path,
    offset: u32,
    limit: u32,
    workspace_root: &Path,
) -> ToolResult {
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

    let contents = match std::fs::read_to_string(&canonical_path) {
        Ok(c) => c,
        Err(e) => return ToolResult::error(format!("failed to read {}: {}", path.display(), e)),
    };

    let lines: Vec<&str> = contents.lines().collect();
    let total = lines.len();
    let req_start = offset.max(1) as usize;
    let lim = limit.clamp(1, MAX_LIMIT) as usize;

    if total == 0 {
        return ToolResult {
            success: true,
            output: format!("(empty file: {})", path.display()),
            diff: None,
            error: None,
            stop_reason: None,
        };
    }

    if req_start > total {
        return ToolResult {
            success: false,
            output: String::new(),
            diff: None,
            error: Some(format!(
                "offset {req_start} is past end of file ({} has {total} lines). Use a smaller offset.",
                path.display()
            )),
            stop_reason: None,
        };
    }

    let req_end = (req_start + lim - 1).min(total);
    let window = expand_read_window(path, &contents, req_start, req_end);
    let start = window.start;
    let end = window.end;

    let slice = &lines[start - 1..end];
    let mut body = slice.join("\n");
    if body.len() > MAX_WINDOW_CHARS {
        body.truncate(MAX_WINDOW_CHARS);
        body.push_str("\n...[char-truncated within this line window]");
    }

    let header = if window.expanded {
        let sym = window
            .symbol
            .as_ref()
            .map(|s| format!("function {}", s.name))
            .unwrap_or_else(|| "enclosing symbol".to_string());
        format!(
            "lines {start}-{end} of {total} — {} (expanded from requested {req_start}-{req_end} to cover {sym})",
            path.display()
        )
    } else {
        format!("lines {start}-{end} of {total} — {}", path.display())
    };

    let footer = if end < total {
        format!(
            "\n...[more below — next: read_file path=\"{}\" offset={} limit={}]",
            path.display(),
            end + 1,
            lim
        )
    } else {
        String::new()
    };

    ToolResult {
        success: true,
        output: format!("{header}\n{body}{footer}"),
        diff: None,
        error: None,
        stop_reason: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn ranged_read_with_continuation() {
        let dir = tempfile::tempdir().unwrap();
        let text: String = (1..=20).map(|i| format!("line{i}\n")).collect();
        fs::write(dir.path().join("a.txt"), text).unwrap();

        let result = execute(Path::new("a.txt"), 1, 5, dir.path());
        assert!(result.success, "{:?}", result.error);
        assert!(result.output.contains("lines 1-5 of 20"));
        assert!(result.output.contains("line1"));
        assert!(result.output.contains("line5"));
        assert!(!result.output.contains("line6\n"));
        assert!(result.output.contains("offset=6"));
    }

    #[test]
    fn last_page_has_no_continuation() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "a\nb\nc\n").unwrap();
        let result = execute(Path::new("a.txt"), 2, 10, dir.path());
        assert!(result.success);
        assert!(result.output.contains("lines 2-3 of 3"));
        assert!(!result.output.contains("more below"));
    }

    #[test]
    fn offset_past_end_errors() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "only\n").unwrap();
        let result = execute(Path::new("a.txt"), 5, 10, dir.path());
        assert!(!result.success);
        assert!(result.stop_reason.is_none());
    }

    #[test]
    fn expands_to_full_function() {
        let dir = tempfile::tempdir().unwrap();
        let src = r#"
export async function getTasks(date: string) {
  const x = 1;
  const y = 2;
  return request(`/tasks/?dueDate=${encodeURIComponent(date)}`);
}
"#;
        fs::write(dir.path().join("api.ts"), src).unwrap();
        let result = execute(Path::new("api.ts"), 4, 2, dir.path());
        assert!(result.success, "{:?}", result.error);
        assert!(
            result.output.contains("expanded from requested"),
            "{}",
            result.output
        );
        assert!(result.output.contains("getTasks"), "{}", result.output);
        assert!(result.output.contains("dueDate"), "{}", result.output);
        assert!(
            result.output.contains("encodeURIComponent"),
            "{}",
            result.output
        );
    }
}
