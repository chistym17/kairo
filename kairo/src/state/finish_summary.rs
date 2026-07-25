//! Ground finish summaries in tool-produced diffs (exact identifiers).

use crate::state::agent_state::AgentState;
use crate::state::step::Action;

/// Append an authoritative excerpt of successful apply_patch/write_file diffs so
/// finish prose cannot invent wrong identifiers (e.g. `dueDate` vs `due_date`).
pub fn ground_finish_summary(state: &AgentState, summary: &str) -> String {
    let mut blocks: Vec<String> = Vec::new();
    for step in &state.history {
        if !step.result.success {
            continue;
        }
        let path = match &step.action {
            Action::ApplyPatch { path, .. } | Action::WriteFile { path, .. } => path,
            _ => continue,
        };
        let Some(diff) = step.result.diff.as_deref() else {
            continue;
        };
        if diff.trim().is_empty() {
            continue;
        }
        let excerpt = truncate_diff_excerpt(diff, 40);
        blocks.push(format!("### {}\n```diff\n{excerpt}\n```", path.display()));
    }

    if blocks.is_empty() {
        return summary.to_string();
    }

    format!(
        "{}\n\n## Applied changes (from tool diffs — use these exact identifiers)\n\n{}",
        summary.trim_end(),
        blocks.join("\n\n")
    )
}

fn truncate_diff_excerpt(diff: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = diff.lines().collect();
    if lines.len() <= max_lines {
        return diff.trim_end().to_string();
    }
    format!(
        "{}\n...[{} more diff lines truncated]",
        lines[..max_lines].join("\n"),
        lines.len() - max_lines
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::step::Step;
    use crate::tools::ToolResult;
    use std::path::PathBuf;

    #[test]
    fn appends_diff_identifiers_from_history() {
        let mut state = AgentState::new("goal", "/tmp/ws", "model", 10);
        let diff = "\
--- a/api.ts
+++ b/api.ts
@@ -1 +1 @@
-    request(`/tasks/?due_date=${date}`),
+    request(`/tasks/?date=${date}`),
";
        state.record_step(Step::new(
            0,
            0,
            Action::ApplyPatch {
                path: PathBuf::from("frontend-v2/src/lib/api.ts"),
                diff: diff.into(),
            },
            ToolResult {
                success: true,
                output: "ok".into(),
                diff: Some(diff.into()),
                error: None,
                stop_reason: None,
            },
            "tc1",
        ));

        let out = ground_finish_summary(
            &state,
            "Fixed mismatch. Was dueDate, now date.",
        );
        assert!(out.contains("due_date"), "{out}");
        assert!(out.contains("Applied changes"), "{out}");
        assert!(out.contains("frontend-v2/src/lib/api.ts"), "{out}");
    }
}
