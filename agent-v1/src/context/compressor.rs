/// Simple tool output compressor used only for the LLM context view.
///
/// Phase 6.3 goals:
/// - keep errors / failures readable
/// - keep the last lines (often contain summary)
/// - cap size to avoid blowing the context budget

const MAX_CHARS: usize = 8_000;
const MAX_LINES: usize = 200;
const KEEP_TAIL_LINES: usize = 40;

pub fn compress_tool_output(raw: &str) -> String {
    // Fast path: small content stays untouched.
    if raw.len() <= MAX_CHARS && raw.lines().count() <= MAX_LINES {
        return raw.to_string();
    }

    let lines: Vec<&str> = raw.lines().collect();
    let total_lines = lines.len();

    // Keep "important" lines anywhere + always keep the tail.
    let mut kept: Vec<&str> = Vec::new();
    for line in &lines {
        if is_important_line(line) {
            kept.push(line);
        }
    }

    let tail_start = total_lines.saturating_sub(KEEP_TAIL_LINES);
    for line in &lines[tail_start..] {
        kept.push(line);
    }

    // De-dup while keeping order (simple O(n^2) is fine at these sizes).
    let mut unique: Vec<&str> = Vec::new();
    for line in kept {
        if !unique.contains(&line) {
            unique.push(line);
        }
    }

    let mut out = String::new();
    out.push_str(&format!(
        "[output truncated: {} lines, {} chars]\n",
        total_lines,
        raw.len()
    ));

    // Hard cap output size.
    for line in unique {
        if out.len() + line.len() + 1 > MAX_CHARS {
            out.push_str("[truncated]\n");
            break;
        }
        out.push_str(line);
        out.push('\n');
    }

    // If we accidentally produced something empty (shouldn't), fall back to tail.
    if out.trim().is_empty() {
        let tail = lines[tail_start..].join("\n");
        return format!(
            "[output truncated: {} lines, {} chars]\n{}",
            total_lines,
            raw.len(),
            tail
        );
    }

    out.trim_end().to_string()
}

fn is_important_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.contains("error")
        || lower.contains("failed")
        || lower.contains("panic")
        || lower.contains("traceback")
        || lower.contains("assert")
        || lower.contains("warning")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_output_unchanged() {
        let s = "ok\n";
        assert_eq!(compress_tool_output(s), s);
    }

    #[test]
    fn large_output_gets_truncated_and_keeps_tail() {
        let mut s = String::new();
        for i in 0..500 {
            s.push_str(&format!("line {i}\n"));
        }
        s.push_str("FAILED test_x\n");

        let out = compress_tool_output(&s);
        assert!(out.contains("output truncated"));
        assert!(out.contains("FAILED"));
        assert!(out.contains("line 499"));
    }
}

