//! Structured workspace summary for the model (objective state only).

use crate::state::agent_state::AgentState;
use crate::state::completion::CompletionTracker;
use crate::tools::git_diff::{self, GitDiffContext};

/// Compact post-edit workspace state injected into the model goal.
pub fn render_workspace_summary(state: &AgentState) -> String {
    let mut files: Vec<String> = state
        .governance
        .files_ever_modified
        .iter()
        .cloned()
        .collect();
    files.sort();

    let files_line = if files.is_empty() {
        "(none yet)".to_string()
    } else {
        files.join(", ")
    };

    let diff_summary = summarize_diff_vs_head(state, &files);
    let (verify_status, verify_blockers) = verification_lines(&state.completion);
    let plan_line = unresolved_plan_items(state);

    format!(
        "Files changed this run: {files_line}\n\
         Current diff versus HEAD: {diff_summary}\n\
         Verification status: {verify_status}\n\
         Verification blockers: {verify_blockers}\n\
         Unresolved task items: {plan_line}"
    )
}

fn summarize_diff_vs_head(state: &AgentState, files: &[String]) -> String {
    if files.is_empty() {
        return "no run edits yet".to_string();
    }

    let ctx = GitDiffContext {
        files_ever_modified: files.to_vec(),
    };
    let result = git_diff::execute(&None, &state.workspace_root, Some(&ctx));
    if !result.success {
        return format!(
            "unavailable ({})",
            result.error.unwrap_or_else(|| "error".into())
        );
    }

    let out = result.output.trim();
    if out.contains("clean relative to HEAD") || out == "(no diff)" || out == "(clean — no changes)"
    {
        return "clean — run edits match HEAD (or were reverted)".to_string();
    }

    // Keep a short preview so the model sees remaining hunks exist.
    let preview: String = out.lines().take(12).collect::<Vec<_>>().join("\n");
    if out.lines().count() > 12 {
        format!("{preview}\n…")
    } else {
        preview
    }
}

fn verification_lines(c: &CompletionTracker) -> (String, String) {
    if let Some(outcome) = &c.last_verification {
        let exit = outcome
            .exit_code
            .map(|code| format!(", exit {code}"))
            .unwrap_or_default();
        use crate::verification::VerificationStatus;
        match outcome.status {
            VerificationStatus::Passed => (
                format!(
                    "{} passed ({:?}{exit}): {}",
                    outcome.check_id, outcome.kind, outcome.summary
                ),
                "none".into(),
            ),
            VerificationStatus::EnvironmentBlocked => (
                format!(
                    "{} environment_blocked ({:?}{exit}) — not a patch failure",
                    outcome.check_id, outcome.kind
                ),
                outcome.summary.clone(),
            ),
            VerificationStatus::Failed => (
                format!("{} failed ({:?}{exit})", outcome.check_id, outcome.kind),
                outcome.summary.clone(),
            ),
        }
    } else {
        match c.last_verify_success {
            None if c.last_edit_turn.is_none() => ("not started".into(), "none".into()),
            None => ("not attempted since last edit".into(), "none".into()),
            Some(true) => (
                "last attempt reported success".into(),
                "none".into(),
            ),
            Some(false) => (
                "last attempt failed".into(),
                c.last_verify_detail
                    .clone()
                    .unwrap_or_else(|| "command failed".into()),
            ),
        }
    }
}

fn unresolved_plan_items(state: &AgentState) -> String {
    let Some(plan) = &state.plan else {
        return "no plan".to_string();
    };
    let open: Vec<&str> = plan
        .steps
        .iter()
        .filter(|s| !s.done)
        .map(|s| s.text.as_str())
        .collect();
    if open.is_empty() {
        "none (plan complete)".to_string()
    } else {
        open.join("; ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::agent_state::AgentState;
    use std::path::PathBuf;

    #[test]
    fn renders_without_edits() {
        let state = AgentState::new("g", PathBuf::from("."), "m", 10);
        let s = render_workspace_summary(&state);
        assert!(s.contains("Files changed this run: (none yet)"));
        assert!(s.contains("Verification status:"));
    }
}
