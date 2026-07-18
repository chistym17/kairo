//! Expand a requested line window to cover the enclosing code symbol.

use std::path::Path;

use crate::repo_intel::{CompositeSymbolExtractor, Language};
use crate::tools::read_file::MAX_LIMIT;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolSpan {
    pub name: String,
    pub start_line: usize,
    pub end_line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpandedWindow {
    pub start: usize,
    pub end: usize,
    pub symbol: Option<SymbolSpan>,
    /// True when we grew beyond the caller's requested window.
    pub expanded: bool,
}

/// If the request sits inside a detected symbol, expand to that symbol's span
/// (clamped to [`MAX_LIMIT`] lines). Otherwise return the original window.
pub fn expand_read_window(
    path: &Path,
    content: &str,
    req_start: usize,
    req_end: usize,
) -> ExpandedWindow {
    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();
    if total == 0 || req_start == 0 {
        return ExpandedWindow {
            start: req_start.max(1),
            end: req_end.max(1),
            symbol: None,
            expanded: false,
        };
    }

    let req_start = req_start.clamp(1, total);
    let req_end = req_end.clamp(req_start, total);

    let Some(span) = find_enclosing_symbol(path, content, &lines, req_start, req_end) else {
        return ExpandedWindow {
            start: req_start,
            end: req_end,
            symbol: None,
            expanded: false,
        };
    };

    let mut start = req_start.min(span.start_line);
    let mut end = req_end.max(span.end_line);
    start = start.clamp(1, total);
    end = end.clamp(start, total);

    let span_len = end - start + 1;
    if span_len > MAX_LIMIT as usize {
        // Prefer keeping the symbol start; fill up to MAX_LIMIT.
        start = span.start_line.clamp(1, total);
        end = (start + MAX_LIMIT as usize - 1).min(span.end_line).min(total);
        if end < req_end && end - start + 1 < MAX_LIMIT as usize {
            end = (start + MAX_LIMIT as usize - 1).min(total);
        }
    }

    let expanded = start < req_start || end > req_end;
    ExpandedWindow {
        start,
        end,
        symbol: Some(span),
        expanded,
    }
}

fn find_enclosing_symbol(
    path: &Path,
    content: &str,
    lines: &[&str],
    req_start: usize,
    req_end: usize,
) -> Option<SymbolSpan> {
    let extractor = CompositeSymbolExtractor::default();
    let symbols = extractor.extract_for_path(path, content);
    if symbols.is_empty() {
        return None;
    }

    // Innermost definition that starts at or before the requested window.
    let mut best: Option<(usize, String)> = None; // (start_line, name)
    for sym in &symbols {
        if sym.line == 0 || sym.line > req_end {
            continue;
        }
        if sym.line <= req_start {
            match &best {
                None => best = Some((sym.line, sym.name.clone())),
                Some((prev, _)) if sym.line >= *prev => best = Some((sym.line, sym.name.clone())),
                _ => {}
            }
        }
    }

    // If the window starts mid-file before any symbol but overlaps a later body,
    // also consider the nearest symbol starting inside the window.
    if best.is_none() {
        for sym in &symbols {
            if sym.line >= req_start && sym.line <= req_end {
                match &best {
                    None => best = Some((sym.line, sym.name.clone())),
                    Some((prev, _)) if sym.line < *prev => {
                        best = Some((sym.line, sym.name.clone()))
                    }
                    _ => {}
                }
            }
        }
    }

    let (start_line, name) = best?;
    let lang = Language::from_path(path);
    let end_line = match lang {
        Language::Python => python_block_end(lines, start_line)?,
        _ => brace_block_end(lines, start_line)
            .or_else(|| python_block_end(lines, start_line))?,
    };

    // Only expand when the request actually intersects the symbol body.
    if req_end < start_line || req_start > end_line {
        return None;
    }

    Some(SymbolSpan {
        name,
        start_line,
        end_line,
    })
}

fn brace_block_end(lines: &[&str], start_line: usize) -> Option<usize> {
    let start_idx = start_line.saturating_sub(1);
    let mut depth = 0i32;
    let mut seen_open = false;
    let mut in_line_comment = false;
    let mut in_block_comment = false;
    let mut in_string: Option<char> = None;

    for (i, line) in lines.iter().enumerate().skip(start_idx) {
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            if in_line_comment {
                break;
            }
            if in_block_comment {
                if c == '*' && chars.peek() == Some(&'/') {
                    chars.next();
                    in_block_comment = false;
                }
                continue;
            }
            if let Some(q) = in_string {
                if c == '\\' {
                    chars.next();
                    continue;
                }
                if c == q {
                    in_string = None;
                }
                continue;
            }
            match c {
                '/' if chars.peek() == Some(&'/') => {
                    in_line_comment = true;
                }
                '/' if chars.peek() == Some(&'*') => {
                    chars.next();
                    in_block_comment = true;
                }
                '"' | '\'' | '`' => in_string = Some(c),
                '{' => {
                    depth += 1;
                    seen_open = true;
                }
                '}' => {
                    if seen_open {
                        depth -= 1;
                        if depth == 0 {
                            return Some(i + 1);
                        }
                    }
                }
                _ => {}
            }
        }
        in_line_comment = false;
    }

    if seen_open {
        // Unclosed — take rest of file as best effort.
        return Some(lines.len());
    }
    None
}

fn python_block_end(lines: &[&str], start_line: usize) -> Option<usize> {
    let start_idx = start_line.saturating_sub(1);
    let header = lines.get(start_idx)?;
    let base_indent = indent_width(header);
    let mut end = start_line;

    for (i, line) in lines.iter().enumerate().skip(start_idx + 1) {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            end = i + 1;
            continue;
        }
        let ind = indent_width(line);
        if ind <= base_indent {
            break;
        }
        end = i + 1;
    }
    Some(end.max(start_line))
}

fn indent_width(line: &str) -> usize {
    line.chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .map(|c| if c == '\t' { 4 } else { 1 })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_partial_ts_function_read() {
        let content = r#"
export async function getTasks(date: string) {
  const a = 1;
  const b = 2;
  return request(`/tasks/?dueDate=${encodeURIComponent(date)}`);
}

export function other() {
  return 1;
}
"#;
        // Request middle of getTasks (lines are 1-based; leading newline → line1 empty)
        let lines: Vec<&str> = content.lines().collect();
        let start_fn = lines
            .iter()
            .position(|l| l.contains("function getTasks"))
            .unwrap()
            + 1;
        let mid = start_fn + 2;
        let win = expand_read_window(Path::new("api.ts"), content, mid, mid + 1);
        assert!(win.expanded, "{win:?}");
        let sym = win.symbol.as_ref().unwrap();
        assert_eq!(sym.name, "getTasks");
        assert!(win.start <= start_fn);
        assert!(win.end > mid + 1);
        assert!(content.lines().nth(win.end - 1).unwrap().contains('}') || win.end > mid);
    }

    #[test]
    fn no_expand_outside_symbols() {
        let content = "const x = 1;\nconst y = 2;\n";
        let win = expand_read_window(Path::new("a.ts"), content, 1, 2);
        assert!(!win.expanded);
    }
}
