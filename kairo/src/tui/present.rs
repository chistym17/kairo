//! TUI-only presentation filter.
//!
//! Keeps the conversation/activity panels product-facing: plan, real file work,
//! and a clean final answer — not harness denials, monitor transitions, or raw
//! stop-reason debug strings. Classic terminal output is unchanged.

use crate::api::{RunResult, RunStatus};
use crate::cli::humanize_error;
use crate::cli::output::{tool_label, user_facing_stop_message};

/// Tools users care about seeing in the Activity strip.
fn is_user_facing_tool(tool: &str) -> bool {
    matches!(
        tool.trim(),
        "read_file"
            | "write_file"
            | "apply_patch"
            | "search_code"
            | "locate"
            | "grep"
            | "run_shell"
            | "run_command"
            | "git_status"
            | "git_diff"
            | "list_dir"
            | "list_files"
            | "finish"
    )
}

/// Synthetic / bookkeeping tools the model or harness emits — never show.
fn is_internal_tool(tool: &str) -> bool {
    matches!(
        tool.trim(),
        "no_tool_call" | "update_plan" | "step" | "thinking"
    )
}

/// Summaries that smell like runtime/monitor enforcement, not user work.
fn looks_like_harness_noise(text: &str) -> bool {
    let l = text.to_ascii_lowercase();
    l.contains("duplicate read denied")
        || l.contains("exact content already available")
        || l.contains("phase_tool_denied")
        || l.contains("authorization denied")
        || l.contains("denied write_file")
        || l.contains("denied_source_writes")
        || l.contains("evidence_checkpoint")
        || l.contains("ungrounded")
        || l.contains("harness")
        || l.contains("monitor")
        || l.contains("[transition]")
        || l.contains("[state]")
        || l.contains("stall")
        || l.contains("recovery_active")
        || l.contains("legacy_decision")
        || l.contains("candidate_decision")
}

/// Spinner text while a tool is in flight. `None` = keep quiet.
pub fn tool_started_label(tool: &str, summary: &str) -> Option<String> {
    if is_internal_tool(tool) || !is_user_facing_tool(tool) {
        return None;
    }
    if looks_like_harness_noise(summary) {
        return None;
    }
    Some(tool_label(tool, summary))
}

/// Milestone after a tool finishes. `None` = hide from Activity.
pub fn tool_completed_milestone(tool: &str, success: bool, summary: &str) -> Option<String> {
    if is_internal_tool(tool) || !is_user_facing_tool(tool) {
        return None;
    }
    if looks_like_harness_noise(summary) {
        return None;
    }

    // Failed reads/searches/lists are almost always harness denials in this
    // runtime — hide them. Real shell/edit failures still surface.
    if !success {
        let hide_failures = matches!(
            tool.trim(),
            "read_file"
                | "search_code"
                | "locate"
                | "grep"
                | "list_dir"
                | "list_files"
                | "git_status"
                | "git_diff"
                | "finish"
        );
        if hide_failures {
            return None;
        }
        let label = tool_label(tool, summary);
        return Some(format!("✗ {label}"));
    }

    // Finish is reflected in the result card; skip the tool tick.
    if tool.trim() == "finish" {
        return None;
    }

    // Writes/patches get a path via FileChanged — avoid duplicate "edit" lines.
    if matches!(tool.trim(), "write_file" | "apply_patch") {
        return None;
    }

    let label = tool_label(tool, summary);
    Some(format!("✓ {label}"))
}

/// File mutation line for Activity (Changes tab is updated separately).
pub fn file_changed_milestone(path: &str) -> String {
    format!("✓ edit {path}")
}

pub fn verification_milestone(command: &str, success: bool, summary: &str) -> String {
    let mark = if success { "✓" } else { "✗" };
    if summary.is_empty() || summary.eq_ignore_ascii_case("ok") {
        format!("{mark} verify {command}")
    } else {
        format!("{mark} verify {command} — {summary}")
    }
}

/// Friendly conversation / result copy for errors.
pub fn friendly_error(message: &str) -> String {
    if looks_like_harness_noise(message) {
        return "Something went wrong — try again.".into();
    }
    humanize_error(message)
}

/// Result-card title + summary for a finished run.
pub fn finished_card(result: &RunResult) -> (String, String) {
    match result.status {
        RunStatus::Completed => {
            let summary = result
                .summary
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty() && !looks_like_harness_noise(s))
                .unwrap_or("Done.")
                .to_string();
            ("Completed — Verified".into(), summary)
        }
        RunStatus::CompletedUnverified => {
            let summary = result
                .summary
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty() && !looks_like_harness_noise(s))
                .unwrap_or("Done (unverified).")
                .to_string();
            ("Completed — Unverified".into(), summary)
        }
        RunStatus::Stopped => {
            let why = user_facing_stop_message(result);
            let why = if looks_like_harness_noise(&why) {
                "Couldn't finish — try again.".into()
            } else {
                why
            };
            ("Couldn't finish".into(), why)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::RunUsage;
    use std::path::PathBuf;

    #[test]
    fn hides_duplicate_read_failures() {
        assert!(tool_completed_milestone(
            "read_file",
            false,
            "read_file failed"
        )
        .is_none());
        assert!(tool_completed_milestone(
            "read_file",
            false,
            "Duplicate read denied: …"
        )
        .is_none());
    }

    #[test]
    fn shows_successful_reads() {
        let line = tool_completed_milestone("read_file", true, "read_file src/a.rs").unwrap();
        assert!(line.contains('✓'));
        assert!(line.contains("read"));
    }

    #[test]
    fn hides_synthetic_no_tool_call() {
        assert!(tool_started_label("no_tool_call", "no_tool_call").is_none());
        assert!(tool_completed_milestone("no_tool_call", false, "failed").is_none());
        assert!(tool_completed_milestone("update_plan", true, "ok").is_none());
    }

    #[test]
    fn shows_shell_failures() {
        let line = tool_completed_milestone("run_shell", false, "run_shell cargo test").unwrap();
        assert!(line.starts_with('✗'));
    }

    #[test]
    fn stopped_run_is_friendly() {
        let (title, summary) = finished_card(&RunResult {
            run_id: "r".into(),
            status: RunStatus::Stopped,
            success: false,
            summary: None,
            changed_files: vec![],
            steps: 8,
            model_turns: 5,
            duration_ms: 1000,
            stop_reason: "NoProgress".into(),
            log_path: PathBuf::new(),
            usage: RunUsage::default(),
        });
        assert_eq!(title, "Couldn't finish");
        assert!(!summary.contains("NoProgress"));
        assert!(summary.to_ascii_lowercase().contains("progress") || summary.contains("try"));
    }
}
