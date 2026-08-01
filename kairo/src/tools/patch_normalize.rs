//! Safe bare-hunk header normalization before `mpatch` dispatch.
//!
//! Models sometimes emit valid unified-diff bodies with incomplete headers:
//!
//! ```text
//! @@
//!  context
//! -old
//! +new
//! ```
//!
//! `mpatch` expects numbered headers (`@@ -N,M +N,M @@`). This module rewrites
//! bare headers **only** when the old context/removal block matches exactly once
//! in the target file. Ambiguous or weak anchors are rejected — never guessed.

use std::path::Path;

/// Why a bare hunk could not be safely rewritten.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NormalizeError {
    TargetUnreadable { path: String, detail: String },
    Ambiguous { path: String, locations: usize },
    NoMatch { path: String },
    NoOldContext { path: String },
    WeakContext { path: String },
    MixedMalformed { path: String, detail: String },
}

impl NormalizeError {
    pub fn user_message(&self) -> String {
        match self {
            Self::TargetUnreadable { path, detail } => format!(
                "Bare hunk header could not be normalized.\n\n\
                 Target: {path}\n\
                 Reason: could not read target file ({detail}).\n\n\
                 Retry with a numbered unified-diff header or include more unchanged context."
            ),
            Self::Ambiguous { path, locations } => format!(
                "Bare hunk header could not be normalized.\n\n\
                 Target: {path}\n\
                 Reason: old context matched {locations} locations.\n\n\
                 Retry with a numbered unified-diff header or include more unchanged context."
            ),
            Self::NoMatch { path } => format!(
                "Bare hunk header could not be normalized.\n\n\
                 Target: {path}\n\
                 Reason: old context matched 0 locations.\n\n\
                 Retry with a numbered unified-diff header or include more unchanged context."
            ),
            Self::NoOldContext { path } => format!(
                "Bare hunk header could not be normalized.\n\n\
                 Target: {path}\n\
                 Reason: hunk has no old context or removal lines to anchor on.\n\n\
                 Retry with a numbered unified-diff header or include more unchanged context."
            ),
            Self::WeakContext { path } => format!(
                "Bare hunk header could not be normalized.\n\n\
                 Target: {path}\n\
                 Reason: old context is too weak to locate safely.\n\n\
                 Retry with a numbered unified-diff header or include more unchanged context."
            ),
            Self::MixedMalformed { path, detail } => format!(
                "Bare hunk header could not be normalized.\n\n\
                 Target: {path}\n\
                 Reason: {detail}.\n\n\
                 Retry with a numbered unified-diff header or include more unchanged context."
            ),
        }
    }
}

/// Normalize bare `@@` hunk headers in `diff` against `workspace_root/path`.
///
/// Returns:
/// - `Ok(None)` when every hunk is already numbered (diff unchanged)
/// - `Ok(Some(normalized))` when at least one bare hunk was safely rewritten
/// - `Err(...)` when any bare hunk cannot be uniquely grounded
pub fn normalize_bare_hunks(
    path: &Path,
    diff: &str,
    workspace_root: &Path,
) -> Result<Option<String>, NormalizeError> {
    let path_display = path.display().to_string();
    if !diff.lines().any(is_bare_hunk_header) {
        return Ok(None);
    }

    let full_path = workspace_root.join(path);
    let content = std::fs::read_to_string(&full_path).map_err(|e| {
        NormalizeError::TargetUnreadable {
            path: path_display.clone(),
            detail: e.to_string(),
        }
    })?;
    let file_lines: Vec<&str> = content.lines().collect();

    let mut out = String::with_capacity(diff.len() + 32);
    let mut changed = false;
    let mut lines = diff.lines().peekable();
    let ends_with_newline = diff.ends_with('\n');

    while let Some(line) = lines.next() {
        if is_numbered_hunk_header(line) {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if is_bare_hunk_header(line) {
            let mut body: Vec<&str> = Vec::new();
            while let Some(next) = lines.peek().copied() {
                if next.starts_with("@@") {
                    break;
                }
                // Next file header ends this hunk.
                if (next.starts_with("--- ") || next.starts_with("+++ ")) && !body.is_empty() {
                    break;
                }
                body.push(lines.next().unwrap());
            }

            let header = rewrite_bare_hunk(&path_display, &file_lines, &body)?;
            out.push_str(&header);
            out.push('\n');
            for b in &body {
                out.push_str(b);
                out.push('\n');
            }
            changed = true;
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }

    if !changed {
        // Had a line starting with @@ that wasn't numbered or bare-recognized.
        return Err(NormalizeError::MixedMalformed {
            path: path_display,
            detail: "malformed hunk header structure".into(),
        });
    }

    if !ends_with_newline {
        out.pop();
    }
    Ok(Some(out))
}

fn is_bare_hunk_header(line: &str) -> bool {
    let trimmed = line.trim_end();
    if !trimmed.starts_with("@@") {
        return false;
    }
    if is_numbered_hunk_header(trimmed) {
        return false;
    }
    // `@@` or `@@ optional heading` (no -N +M range).
    let rest = trimmed.strip_prefix("@@").unwrap_or("").trim();
    if rest.is_empty() {
        return true;
    }
    // Heading text without ranges, e.g. `@@ def get_shapes`.
    !rest.starts_with('-')
}

fn is_numbered_hunk_header(line: &str) -> bool {
    let trimmed = line.trim_end();
    let Some(rest) = trimmed.strip_prefix("@@") else {
        return false;
    };
    let rest = rest.trim_start();
    // -start[,count] +start[,count]
    let mut parts = rest.split_whitespace();
    let Some(old) = parts.next() else {
        return false;
    };
    let Some(new) = parts.next() else {
        return false;
    };
    parse_range_token(old, '-') && parse_range_token(new, '+')
}

fn parse_range_token(token: &str, expected_sign: char) -> bool {
    let Some(rest) = token.strip_prefix(expected_sign) else {
        return false;
    };
    let (start, count) = match rest.split_once(',') {
        Some((s, c)) => (s, Some(c)),
        None => (rest, None),
    };
    if start.is_empty() || !start.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    if let Some(c) = count {
        if c.is_empty() || !c.chars().all(|ch| ch.is_ascii_digit()) {
            return false;
        }
    }
    true
}

fn rewrite_bare_hunk(
    path_display: &str,
    file_lines: &[&str],
    body: &[&str],
) -> Result<String, NormalizeError> {
    let mut old_block: Vec<&str> = Vec::new();
    let mut old_count = 0usize;
    let mut new_count = 0usize;
    let mut has_nonempty_anchor = false;

    for line in body {
        if line.starts_with('\\') {
            // "\ No newline at end of file"
            continue;
        }
        if let Some(rest) = line.strip_prefix('+').filter(|r| !r.starts_with('+')) {
            new_count += 1;
            let _ = rest;
            continue;
        }
        if let Some(rest) = line.strip_prefix('-').filter(|r| !r.starts_with('-')) {
            old_block.push(rest);
            old_count += 1;
            if !rest.trim().is_empty() {
                has_nonempty_anchor = true;
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix(' ') {
            old_block.push(rest);
            old_count += 1;
            new_count += 1;
            if !rest.trim().is_empty() {
                has_nonempty_anchor = true;
            }
            continue;
        }
        // Unprefixed body line — treat as context (some models omit the space).
        if !line.starts_with("---") && !line.starts_with("+++") && !line.starts_with("@@") {
            old_block.push(line);
            old_count += 1;
            new_count += 1;
            if !line.trim().is_empty() {
                has_nonempty_anchor = true;
            }
        }
    }

    if old_block.is_empty() || old_count == 0 {
        return Err(NormalizeError::NoOldContext {
            path: path_display.to_string(),
        });
    }
    if !has_nonempty_anchor {
        return Err(NormalizeError::WeakContext {
            path: path_display.to_string(),
        });
    }

    let hits = find_exact_block(file_lines, &old_block);
    match hits.len() {
        0 => Err(NormalizeError::NoMatch {
            path: path_display.to_string(),
        }),
        1 => {
            let start = hits[0];
            Ok(format!(
                "@@ -{start},{old_count} +{start},{new_count} @@"
            ))
        }
        n => Err(NormalizeError::Ambiguous {
            path: path_display.to_string(),
            locations: n,
        }),
    }
}

/// 1-based start line indices where `block` matches exactly.
fn find_exact_block(file_lines: &[&str], block: &[&str]) -> Vec<usize> {
    if block.is_empty() || block.len() > file_lines.len() {
        return Vec::new();
    }
    let mut hits = Vec::new();
    let last = file_lines.len() - block.len();
    for i in 0..=last {
        if file_lines[i..i + block.len()] == *block {
            hits.push(i + 1);
        }
    }
    hits
}

/// When normalization found a unique old-block start, expose it for later
/// grounding packets (Slice 5G follow-up). Returns 1-based inclusive range.
pub fn unique_old_block_range(
    path: &Path,
    diff: &str,
    workspace_root: &Path,
) -> Option<(usize, usize)> {
    let content = std::fs::read_to_string(workspace_root.join(path)).ok()?;
    let file_lines: Vec<&str> = content.lines().collect();
    let mut lines = diff.lines().peekable();
    let mut best: Option<(usize, usize)> = None;

    while let Some(line) = lines.next() {
        if !is_bare_hunk_header(line) && !is_numbered_hunk_header(line) {
            continue;
        }
        let mut body: Vec<&str> = Vec::new();
        while let Some(next) = lines.peek().copied() {
            if next.starts_with("@@") {
                break;
            }
            if (next.starts_with("--- ") || next.starts_with("+++ ")) && !body.is_empty() {
                break;
            }
            body.push(lines.next().unwrap());
        }
        let mut old_block: Vec<&str> = Vec::new();
        for b in &body {
            if b.starts_with('\\') {
                continue;
            }
            if b.starts_with('+') && !b.starts_with("+++") {
                continue;
            }
            if let Some(rest) = b.strip_prefix('-').filter(|r| !r.starts_with('-')) {
                old_block.push(rest);
            } else if let Some(rest) = b.strip_prefix(' ') {
                old_block.push(rest);
            }
        }
        if old_block.is_empty() {
            continue;
        }
        let hits = find_exact_block(&file_lines, &old_block);
        if hits.len() == 1 {
            let start = hits[0];
            let end = start + old_block.len().saturating_sub(1);
            best = Some(match best {
                None => (start, end),
                Some((s, e)) => (s.min(start), e.max(end)),
            });
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn write_serializers(dir: &Path) {
        fs::write(
            dir.join("serializers.py"),
            "\
class TaskSerializer:
    def get_shapes(self, obj):
        return ShapeSerializer(obj.shapes.all(), many=True).data

    def get_other(self, obj):
        return ShapeSerializer(obj.shapes.all(), many=True).data
",
        )
        .unwrap();
    }

    #[test]
    fn bare_hunk_unique_old_block_gets_numbered_header() {
        let dir = tempdir().unwrap();
        write_serializers(dir.path());
        // First char is the unified-diff marker; remaining text must match file lines exactly.
        let diff = "\
--- serializers.py
+++ serializers.py
@@
     def get_shapes(self, obj):
-        return ShapeSerializer(obj.shapes.all(), many=True).data
+        return ShapeSerializer(obj.shapes.all(), many=True, context=self.context).data
";
        let out = normalize_bare_hunks(Path::new("serializers.py"), diff, dir.path())
            .unwrap()
            .expect("should normalize");
        assert!(
            out.contains("@@ -2,2 +2,2 @@"),
            "expected numbered header, got:\n{out}"
        );
        assert!(!out.lines().any(|l| l.trim() == "@@"));
    }

    #[test]
    fn bare_hunk_applies_via_mpatch_after_normalize() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("serializers.py"),
            "\
class TaskSerializer:
    def get_shapes(self, obj):
        return ShapeSerializer(obj.shapes.all(), many=True).data
",
        )
        .unwrap();
        let diff = "\
--- serializers.py
+++ serializers.py
@@
     def get_shapes(self, obj):
-        return ShapeSerializer(obj.shapes.all(), many=True).data
+        return ShapeSerializer(obj.shapes.all(), many=True, context=self.context).data
";
        let normalized = normalize_bare_hunks(Path::new("serializers.py"), diff, dir.path())
            .unwrap()
            .unwrap();
        let result = crate::tools::apply_patch::execute(
            Path::new("serializers.py"),
            &normalized,
            dir.path(),
        );
        assert!(result.success, "{:?}", result.error);
        let body = fs::read_to_string(dir.path().join("serializers.py")).unwrap();
        assert!(body.contains("context=self.context"));
    }

    #[test]
    fn bare_hunk_duplicate_old_block_rejected() {
        let dir = tempdir().unwrap();
        write_serializers(dir.path());
        // Both get_shapes and get_other share the same return line — use that alone.
        let diff = "\
--- serializers.py
+++ serializers.py
@@
-        return ShapeSerializer(obj.shapes.all(), many=True).data
+        return ShapeSerializer(obj.shapes.all(), many=True, context=self.context).data
";
        let err = normalize_bare_hunks(Path::new("serializers.py"), diff, dir.path()).unwrap_err();
        match err {
            NormalizeError::Ambiguous { locations, .. } => assert!(locations >= 2),
            other => panic!("expected Ambiguous, got {other:?}"),
        }
        let msg = err.user_message();
        assert!(msg.contains("matched 2 locations") || msg.contains("matched"));
        assert!(msg.contains("numbered unified-diff"));
    }

    #[test]
    fn bare_hunk_no_old_lines_rejected() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
        let diff = "\
--- a.txt
+++ a.txt
@@
+brand new line
";
        let err = normalize_bare_hunks(Path::new("a.txt"), diff, dir.path()).unwrap_err();
        assert!(matches!(err, NormalizeError::NoOldContext { .. }));
    }

    #[test]
    fn bare_hunk_whitespace_only_context_rejected() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "\n\nhello\n").unwrap();
        // Only blank context lines — too weak to locate safely.
        let weak = "\
--- a.txt
+++ a.txt
@@
 
 
+inserted
";
        let err = normalize_bare_hunks(Path::new("a.txt"), weak, dir.path()).unwrap_err();
        assert!(matches!(
            err,
            NormalizeError::WeakContext { .. } | NormalizeError::NoOldContext { .. }
        ));
    }

    #[test]
    fn numbered_hunk_unchanged() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "hello world\n").unwrap();
        let diff = "\
--- a.txt
+++ a.txt
@@ -1 +1 @@
-hello world
+hello there
";
        assert!(normalize_bare_hunks(Path::new("a.txt"), diff, dir.path())
            .unwrap()
            .is_none());
    }

    #[test]
    fn multiple_bare_hunks_normalized_independently() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("a.txt"),
            "alpha\nkeep\nbeta\nkeep2\n",
        )
        .unwrap();
        let diff = "\
--- a.txt
+++ a.txt
@@
-alpha
+ALPHA
@@
-beta
+BETA
";
        let out = normalize_bare_hunks(Path::new("a.txt"), diff, dir.path())
            .unwrap()
            .unwrap();
        assert!(out.contains("@@ -1,1 +1,1 @@"), "{out}");
        assert!(out.contains("@@ -3,1 +3,1 @@"), "{out}");
    }

    #[test]
    fn indentation_mismatch_is_exact_no_match() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("a.py"),
            "def f():\n    return 1\n",
        )
        .unwrap();
        // Patch uses 2-space indent on the removal; file has 4 — must not fuzzy-match.
        let diff = "\
--- a.py
+++ a.py
@@
 def f():
-  return 1
+  return 2
";
        let err = normalize_bare_hunks(Path::new("a.py"), diff, dir.path()).unwrap_err();
        assert!(matches!(err, NormalizeError::NoMatch { .. }));
    }

    #[test]
    fn bare_with_heading_text_still_normalized() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "one\ntwo\n").unwrap();
        let diff = "\
--- a.txt
+++ a.txt
@@ function body
-one
+ONE
";
        let out = normalize_bare_hunks(Path::new("a.txt"), diff, dir.path())
            .unwrap()
            .unwrap();
        assert!(out.contains("@@ -1,1 +1,1 @@"), "{out}");
    }
}
