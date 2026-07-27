use crate::state::step::{Action, Step};

/// Bounded view of run step history for LLM context.
#[derive(Debug, Clone)]
pub struct RunContextView {
    pub run_summary: Option<String>,
    pub recent_steps: Vec<Step>,
}

pub fn build_run_context_view(history: &[Step], keep_recent: usize) -> RunContextView {
    if history.len() <= keep_recent {
        return RunContextView {
            run_summary: None,
            recent_steps: history.to_vec(),
        };
    }
    let split = history.len().saturating_sub(keep_recent);
    let older = &history[..split];
    let recent = history[split..].to_vec();
    RunContextView {
        run_summary: Some(summarize_run_steps(older)),
        recent_steps: recent,
    }
}

fn summarize_run_steps(steps: &[Step]) -> String {
    const MAX_CHARS: usize = 2000;
    const MAX_LINES: usize = 40;

    let mut files_inspected = Vec::new();
    let mut files_modified = Vec::new();
    let mut verifications = Vec::new();
    let mut diagnoses = Vec::new();
    let mut blocked = Vec::new();

    for step in steps {
        match &step.action {
            Action::ReadFile { path, .. } => {
                let p = path.to_string_lossy().replace('\\', "/");
                if !files_inspected.contains(&p) {
                    files_inspected.push(p);
                }
            }
            Action::SearchCode { query, path } => {
                diagnoses.push(format!(
                    "searched {:?} in {}",
                    query,
                    path.to_string_lossy()
                ));
            }
            Action::Locate { query, path } => {
                diagnoses.push(format!(
                    "located {:?} in {}",
                    query,
                    path.to_string_lossy()
                ));
            }
            Action::ApplyPatch { path, .. } | Action::WriteFile { path, .. } => {
                let p = path.to_string_lossy().replace('\\', "/");
                if !files_modified.contains(&p) {
                    files_modified.push(p);
                }
            }
            Action::RunCommand { cmd, .. } | Action::RunShell { command: cmd, .. } => {
                let status = if step.result.success && step.result.output.contains("exit_code: 0") {
                    "passed"
                } else if step.result.success {
                    "ran"
                } else {
                    "failed"
                };
                verifications.push(format!("{cmd} ({status})"));
            }
            Action::UpdatePlan { steps, .. } => {
                if steps.contains("[x]") {
                    diagnoses.push(format!("plan: {}", steps.lines().next().unwrap_or("")));
                }
            }
            _ => {}
        }
        if let Some(err) = &step.result.error {
            if err.contains("Blocked") || err.contains("Governance") {
                blocked.push(err.lines().next().unwrap_or(err).to_string());
            }
        }
    }

    let mut lines = vec!["Run summary (older steps):".to_string()];
    if !files_inspected.is_empty() {
        lines.push(format!("- Inspected: {}", files_inspected.join(", ")));
    }
    if !files_modified.is_empty() {
        lines.push(format!("- Modified: {}", files_modified.join(", ")));
    }
    if !diagnoses.is_empty() {
        lines.push(format!("- Investigation: {}", diagnoses.join("; ")));
    }
    if !verifications.is_empty() {
        lines.push(format!(
            "- Verification: {}",
            verifications
                .into_iter()
                .take(5)
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }
    if !blocked.is_empty() {
        lines.push(format!(
            "- Environment blocks: {}",
            blocked.into_iter().take(3).collect::<Vec<_>>().join("; ")
        ));
    }
    if files_modified.is_empty() && !files_inspected.is_empty() {
        lines.push("- No files modified yet in summarized window.".to_string());
    }

    let mut out = lines.join("\n");
    if out.len() > MAX_CHARS {
        out.truncate(MAX_CHARS);
        out.push_str("…");
    }
    if lines.len() > MAX_LINES {
        out.push_str("\n…");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::ToolResult;
    use std::path::PathBuf;

    #[test]
    fn keeps_recent_steps_only() {
        let mut steps = Vec::new();
        for i in 0..15 {
            steps.push(Step::new(
                i,
                i,
                Action::ReadFile {
                    path: PathBuf::from(format!("f{i}.rs")),
                    offset: 1,
                    limit: 10,
                },
                ToolResult {
                    success: true,
                    output: "x".into(),
                    diff: None,
                    error: None,
                    stop_reason: None,
                },
                format!("call_{i}"),
            ));
        }
        let view = build_run_context_view(&steps, 10);
        assert_eq!(view.recent_steps.len(), 10);
        assert!(view.run_summary.is_some());
        assert!(view.run_summary.unwrap().contains("f0.rs"));
    }
}
