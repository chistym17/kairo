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
                error: Some(format_patch_failure(path, patch, &old_content, &e)),
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

/// Build actionable failure text: expected context + closest nearby actual lines.
fn format_patch_failure(
    path: &Path,
    patch: &str,
    old_content: &str,
    engine_err: &impl std::fmt::Display,
) -> String {
    let expected = extract_expected_lines(patch);
    let mut out = format!(
        "Patch hunk did not match {}.\n\
         Engine: {engine_err}\n",
        path.display()
    );

    if !expected.is_empty() {
        out.push_str("\nExpected context / lines to replace:\n");
        for line in expected.iter().take(12) {
            out.push_str(line);
            out.push('\n');
        }
    }

    if let Some((start_line, window)) = find_closest_region(old_content, &expected) {
        let end_line = start_line + window.len().saturating_sub(1);
        out.push_str(&format!(
            "\nActual nearby lines {start_line}–{end_line}:\n"
        ));
        for (i, line) in window.iter().enumerate() {
            out.push_str(&format!("{} | {}\n", start_line + i, line));
        }
    } else if let Some((start_line, window)) = snippet_around_hint(old_content, patch) {
        let end_line = start_line + window.len().saturating_sub(1);
        out.push_str(&format!(
            "\nActual nearby lines {start_line}–{end_line} (from hunk header hint):\n"
        ));
        for (i, line) in window.iter().enumerate() {
            out.push_str(&format!("{} | {}\n", start_line + i, line));
        }
    }

    out.push_str(
        "\nRegenerate the patch using the actual file contents. \
         Prefer read_file on this small range, then apply_patch again. \
         Do not use sed/shell to edit the file.",
    );
    out
}

/// Lines the hunk claims exist (context ` ` and removals `-`), without prefixes.
fn extract_expected_lines(patch: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_hunk = false;
    for line in patch.lines() {
        if line.starts_with("@@") {
            in_hunk = true;
            continue;
        }
        if line.starts_with("---") || line.starts_with("+++") {
            continue;
        }
        if !in_hunk {
            // Allow patches that omit @@ and start with body (rare)
            if line.starts_with(' ') || line.starts_with('-') || line.starts_with('+') {
                in_hunk = true;
            } else {
                continue;
            }
        }
        if line.starts_with('+') && !line.starts_with("+++") {
            continue;
        }
        if let Some(rest) = line.strip_prefix('-').filter(|r| !r.starts_with('-')) {
            out.push(rest.to_string());
        } else if let Some(rest) = line.strip_prefix(' ') {
            out.push(rest.to_string());
        }
    }
    out
}

/// Score windows by ordered line overlap with expected lines.
fn find_closest_region<'a>(
    old_content: &'a str,
    expected: &[String],
) -> Option<(usize, Vec<&'a str>)> {
    if expected.is_empty() {
        return None;
    }
    let file_lines: Vec<&str> = old_content.lines().collect();
    if file_lines.is_empty() {
        return None;
    }

    let needles: Vec<&str> = expected.iter().map(|s| s.as_str()).collect();
    let window = needles.len().clamp(3, 12);
    let mut best: Option<(usize, usize, Vec<&str>)> = None; // score, start, lines

    // Prefer exact multi-line match of the first expected line's neighborhood.
    for (i, line) in file_lines.iter().enumerate() {
        let mut score = 0usize;
        let mut ni = 0usize;
        for fl in file_lines.iter().skip(i).take(window.saturating_mul(2).max(8)) {
            if ni < needles.len() && lines_similar(fl, needles[ni]) {
                score += 2;
                ni += 1;
            } else if needles.iter().any(|n| lines_similar(fl, n)) {
                score += 1;
            }
        }
        // Bonus if first needle appears here
        if needles
            .first()
            .map(|n| lines_similar(line, n) || line.contains(n.trim()) || n.contains(line.trim()))
            .unwrap_or(false)
        {
            score += 3;
        }
        if score == 0 {
            continue;
        }
        let start = i.saturating_sub(1);
        let end = (i + window + 2).min(file_lines.len());
        let snap: Vec<&str> = file_lines[start..end].to_vec();
        match &best {
            None => best = Some((score, start, snap)),
            Some((bs, _, _)) if score > *bs => best = Some((score, start, snap)),
            _ => {}
        }
    }

    // Require a minimum confidence so we don't show random regions.
    let (score, start, snap) = best?;
    if score < 3 {
        return None;
    }
    Some((start + 1, snap)) // 1-based line numbers
}

fn lines_similar(a: &str, b: &str) -> bool {
    let a = a.trim();
    let b = b.trim();
    if a.is_empty() || b.is_empty() {
        return false;
    }
    a == b || a.contains(b) || b.contains(a)
}

/// Fall back to @@ -N,M hunk header when similarity search fails.
fn snippet_around_hint<'a>(old_content: &'a str, patch: &str) -> Option<(usize, Vec<&'a str>)> {
    let file_lines: Vec<&str> = old_content.lines().collect();
    let hint = parse_hunk_old_start(patch)?;
    if hint == 0 {
        return None;
    }
    let start = hint.saturating_sub(1).min(file_lines.len());
    let from = start.saturating_sub(2);
    let to = (start + 8).min(file_lines.len());
    if from >= to {
        return None;
    }
    Some((from + 1, file_lines[from..to].to_vec()))
}

fn parse_hunk_old_start(patch: &str) -> Option<usize> {
    for line in patch.lines() {
        // @@ -193,7 +193,7 @@
        if let Some(rest) = line.strip_prefix("@@") {
            let rest = rest.trim();
            let old = rest.split_whitespace().next()?; // -193,7
            let old = old.trim_start_matches('-');
            let num = old.split(',').next()?;
            return num.parse().ok();
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn failure_shows_actual_nearby_lines() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("api.ts");
        fs::write(
            &path,
            "\n\
export const api = {\n\
  getTasks: (date: string) =>\n\
    request<ApiTask[]>(`/tasks/?due_date=${encodeURIComponent(date)}`),\n\
};\n",
        )
        .unwrap();

        // Completely wrong context that cannot match the file.
        let bad_patch = "\
--- api.ts
+++ api.ts
@@ -1,4 +1,4 @@
 export function totallyMissing() {
-  return client.get('/tasks/', { params: { due_date: date } });
+  return client.get('/tasks/', { params: { date } });
 }
";

        let result = execute(Path::new("api.ts"), bad_patch, dir.path());
        assert!(!result.success, "expected failure, got success: {:?}", result);
        let err = result.error.unwrap();
        assert!(err.contains("Patch hunk did not match"), "{err}");
        assert!(err.contains("Regenerate the patch"), "{err}");
        assert!(
            err.contains("due_date")
                || err.contains("getTasks")
                || err.contains("Expected")
                || err.contains("Actual nearby"),
            "{err}"
        );
    }

    #[test]
    fn successful_patch_still_works() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("a.txt");
        fs::write(&path, "hello world\n").unwrap();
        let patch = "\
--- a.txt
+++ a.txt
@@ -1 +1 @@
-hello world
+hello there
";
        let result = execute(Path::new("a.txt"), patch, dir.path());
        assert!(result.success, "{:?}", result.error);
        assert_eq!(fs::read_to_string(&path).unwrap(), "hello there\n");
    }
}
