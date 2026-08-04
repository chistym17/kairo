//! Map public [`AgentEvent`]s into TUI panel state.

use std::path::Path;
use std::time::Instant;

use crate::api::{AgentEvent, ApprovalRequest, RunResult, RunStatus};

use super::app::{
    App, ChangeEntry, PlanStepStatus, PlanStepView, TimelineItem,
};

/// Apply one public agent event to the shell state.
pub fn apply_event(app: &mut App, event: &AgentEvent) {
    match event {
        AgentEvent::Started {
            run_id: _,
            mode,
            workspace,
            model,
        } => {
            app.header.model = model.clone();
            app.header.workspace = shorten_path(workspace);
            if let Some(ui_mode) = mode_from_str(mode) {
                app.header.mode = ui_mode;
                app.sidebar.session.mode = ui_mode.as_str().to_string();
            }
            app.sidebar.session.model = model.clone();
            app.sidebar.session.workspace = workspace
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("workspace")
                .to_string();
            app.run.begin();
            refresh_status_hint(app);
        }
        AgentEvent::TextDelta { content } => {
            append_assistant(app, content);
        }
        AgentEvent::PlanUpdated {
            version: _,
            objective: _,
            steps,
        } => {
            let mut views = Vec::with_capacity(steps.len());
            let mut done = 0usize;
            let mut marked_active = false;
            for step in steps {
                let status = if step.done {
                    done += 1;
                    PlanStepStatus::Done
                } else if !marked_active {
                    marked_active = true;
                    PlanStepStatus::Active
                } else {
                    PlanStepStatus::Todo
                };
                views.push(PlanStepView {
                    text: step.text.clone(),
                    status,
                });
            }
            app.sidebar.plan_steps = views;
            app.sidebar.plan_done = done;
            app.sidebar.plan_total = steps.len();
        }
        AgentEvent::ToolStarted {
            id: _,
            tool: _,
            summary,
        } => {
            app.run.set_current(summary.clone());
            refresh_status_hint(app);
        }
        AgentEvent::ToolCompleted {
            id: _,
            tool: _,
            success,
            summary,
        } => {
            let mark = if *success { "✓" } else { "✗" };
            app.run.push_milestone(format!("{mark} {summary}"));
            app.run.clear_current();
            refresh_status_hint(app);
        }
        AgentEvent::FileChanged { path } => {
            upsert_change(app, path);
        }
        AgentEvent::VerificationCompleted {
            command,
            success,
            summary,
        } => {
            let mark = if *success { "✓" } else { "✗" };
            let line = if summary.is_empty() {
                format!("{mark} {command}")
            } else {
                format!("{mark} {command} — {summary}")
            };
            app.run.verification.push(line.clone());
            app.run.push_milestone(line);
            app.run.clear_current();
        }
        AgentEvent::ApprovalRequired { request } => {
            app.show_approval(request.clone());
            app.run.set_current(format!("Waiting for approval: {}", request.summary));
            refresh_status_hint(app);
        }
        AgentEvent::Error { message } => {
            app.conversation
                .items
                .push(TimelineItem::Assistant(format!("Error: {message}")));
            app.run.clear_current();
        }
        AgentEvent::Finished { result } => {
            finish_run(app, result);
        }
    }
}

fn finish_run(app: &mut App, result: &RunResult) {
    app.run.busy = false;
    app.run.clear_current();
    app.run.pending_approval = None;
    if matches!(app.overlay, Some(crate::tui::overlay::Overlay::Approval(_))) {
        app.overlay = None;
    }

    for path in &result.changed_files {
        upsert_change(app, path);
    }

    let title = match result.status {
        RunStatus::Completed => "Completed — Verified",
        RunStatus::CompletedUnverified => "Completed — Unverified",
        RunStatus::Stopped => "Stopped",
    };

    let summary = result
        .summary
        .clone()
        .unwrap_or_else(|| result.stop_reason.clone());

    let changed: Vec<String> = app
        .sidebar
        .changes
        .iter()
        .map(|c| format!("M {}  {}", c.path, c.stats))
        .collect();

    let verification = if app.run.verification.is_empty() {
        vec!["(no verification recorded)".into()]
    } else {
        app.run.verification.clone()
    };

    let tokens = format_tokens(
        result.usage.input_tokens.saturating_add(result.usage.output_tokens),
    );
    let elapsed = format_duration_ms(result.duration_ms);
    let footer = format!(
        "{} file{} · {} turns · {tokens} tokens · {elapsed}",
        result.changed_files.len(),
        if result.changed_files.len() == 1 {
            ""
        } else {
            "s"
        },
        result.model_turns,
    );

    app.sidebar.session.tokens = tokens;
    app.sidebar.session.elapsed = elapsed;

    app.conversation.items.push(TimelineItem::ResultCard {
        title: title.into(),
        summary,
        changed,
        verification,
        footer,
    });

    app.run.milestones.clear();
    app.run.verification.clear();
    refresh_status_hint(app);
}

fn append_assistant(app: &mut App, content: &str) {
    match app.conversation.items.last_mut() {
        Some(TimelineItem::Assistant(existing)) => {
            if !existing.is_empty() {
                existing.push('\n');
            }
            existing.push_str(content);
        }
        _ => {
            app.conversation
                .items
                .push(TimelineItem::Assistant(content.to_string()));
        }
    }
}

fn upsert_change(app: &mut App, path: &Path) {
    let display = path.display().to_string();
    if let Some(entry) = app
        .sidebar
        .changes
        .iter_mut()
        .find(|c| c.path == display)
    {
        entry.stats = "changed".into();
        return;
    }
    app.sidebar.changes.push(ChangeEntry {
        path: display,
        stats: "changed".into(),
    });
    if app.sidebar.selected_change >= app.sidebar.changes.len() {
        app.sidebar.selected_change = app.sidebar.changes.len().saturating_sub(1);
    }
}

fn mode_from_str(mode: &str) -> Option<super::app::AgentModeUi> {
    match mode.to_ascii_lowercase().as_str() {
        "ask" => Some(super::app::AgentModeUi::Ask),
        "agent" => Some(super::app::AgentModeUi::Agent),
        "plan" => Some(super::app::AgentModeUi::Plan),
        _ => None,
    }
}

fn shorten_path(path: &Path) -> String {
    crate::cli::display_path(path)
}

pub fn format_tokens(total: u64) -> String {
    if total >= 10_000 {
        format!("{:.1}k", total as f64 / 1000.0)
    } else if total >= 1000 {
        format!("{:.1}k", total as f64 / 1000.0)
    } else {
        total.to_string()
    }
}

pub fn format_duration_ms(ms: u64) -> String {
    let secs = ms / 1000;
    let m = secs / 60;
    let s = secs % 60;
    format!("{m:02}:{s:02}")
}

pub fn refresh_status_hint(app: &mut App) {
    if matches!(app.overlay, Some(crate::tui::overlay::Overlay::Diff(_))) {
        app.status.hint = "↑↓ scroll   Esc close".into();
        return;
    }
    if matches!(app.overlay, Some(crate::tui::overlay::Overlay::Help)) {
        app.status.hint = "Esc close help".into();
        return;
    }
    if let Some(req) = &app.run.pending_approval {
        app.status.hint = format!(
            "Approval: {}   [a] once  [t] task  [d] diff  [r] reject",
            truncate(&req.summary, 36)
        );
        return;
    }
    if app.run.busy {
        let activity = app
            .run
            .current
            .as_deref()
            .unwrap_or("working…");
        app.status.hint = format!("Esc cancel   ·   {activity}");
        return;
    }
    if app.sidebar.tab == crate::tui::app::SidebarTab::Changes
        && !app.sidebar.changes.is_empty()
        && app.composer.is_empty()
    {
        app.status.hint =
            "Enter view diff · ↑↓ select file · Ctrl+P plan · ? help · q quit".into();
        return;
    }
    app.status.hint =
        "Enter send · Shift+Enter newline · ↑↓ history · Esc cancel · Ctrl+L clear · ? help · q quit"
            .into();
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Tick the spinner while a tool is active.
pub fn tick_spinner(app: &mut App) {
    if app.run.current.is_some() {
        app.run.spinner_frame = app.run.spinner_frame.wrapping_add(1);
    }
}

pub fn spinner_glyph(frame: usize) -> char {
    const FRAMES: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    FRAMES[frame % FRAMES.len()]
}

/// Live activity block rendered under the conversation timeline.
#[derive(Debug, Clone, Default)]
pub struct RunUiState {
    pub busy: bool,
    pub current: Option<String>,
    pub milestones: Vec<String>,
    pub verification: Vec<String>,
    pub pending_approval: Option<ApprovalRequest>,
    pub spinner_frame: usize,
    pub started_at: Option<Instant>,
}

impl RunUiState {
    pub fn begin(&mut self) {
        self.busy = true;
        self.current = Some("Starting…".into());
        self.milestones.clear();
        self.verification.clear();
        self.pending_approval = None;
        self.spinner_frame = 0;
        self.started_at = Some(Instant::now());
    }

    pub fn set_current(&mut self, text: String) {
        self.current = Some(text);
        self.busy = true;
    }

    pub fn clear_current(&mut self) {
        self.current = None;
    }

    pub fn push_milestone(&mut self, line: String) {
        // Keep the activity list compact.
        if self.milestones.last() == Some(&line) {
            return;
        }
        self.milestones.push(line);
        if self.milestones.len() > 12 {
            self.milestones.remove(0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{PlanStep, RunUsage};
    use crate::tui::app::App;
    use std::path::PathBuf;

    fn empty_app() -> App {
        App::live_session(
            "deepseek-v4-flash",
            "~/proj",
            "Agent",
            "Confirm",
            "sess1",
            PathBuf::from("/tmp/proj"),
        )
    }

    #[test]
    fn approval_opens_overlay() {
        use crate::api::{ApprovalKind, ApprovalRequest};
        use crate::tui::overlay::Overlay;

        let mut app = empty_app();
        apply_event(
            &mut app,
            &AgentEvent::ApprovalRequired {
                request: ApprovalRequest {
                    id: "ap1".into(),
                    kind: ApprovalKind::FileEdit,
                    summary: "apply_patch src/lib/api.ts".into(),
                    reason: "needs approval".into(),
                    paths: vec![PathBuf::from("src/lib/api.ts")],
                },
            },
        );
        assert!(app.run.pending_approval.is_some());
        assert!(matches!(app.overlay, Some(Overlay::Approval(_))));
    }

    #[test]
    fn plan_updates_sidebar() {
        let mut app = empty_app();
        apply_event(
            &mut app,
            &AgentEvent::PlanUpdated {
                version: 1,
                objective: "fix filter".into(),
                steps: vec![
                    PlanStep {
                        text: "Inspect".into(),
                        done: true,
                    },
                    PlanStep {
                        text: "Patch".into(),
                        done: false,
                    },
                    PlanStep {
                        text: "Verify".into(),
                        done: false,
                    },
                ],
            },
        );
        assert_eq!(app.sidebar.plan_done, 1);
        assert_eq!(app.sidebar.plan_total, 3);
        assert_eq!(app.sidebar.plan_steps[0].status, PlanStepStatus::Done);
        assert_eq!(app.sidebar.plan_steps[1].status, PlanStepStatus::Active);
        assert_eq!(app.sidebar.plan_steps[2].status, PlanStepStatus::Todo);
    }

    #[test]
    fn tools_drive_activity_and_text_builds_assistant() {
        let mut app = empty_app();
        apply_event(
            &mut app,
            &AgentEvent::Started {
                run_id: "r1".into(),
                mode: "agent".into(),
                workspace: PathBuf::from("/tmp/repo"),
                model: "m".into(),
            },
        );
        assert!(app.run.busy);

        apply_event(
            &mut app,
            &AgentEvent::ToolStarted {
                id: "1".into(),
                tool: "read_file".into(),
                summary: "Reading api.ts".into(),
            },
        );
        assert_eq!(app.run.current.as_deref(), Some("Reading api.ts"));

        apply_event(
            &mut app,
            &AgentEvent::ToolCompleted {
                id: "1".into(),
                tool: "read_file".into(),
                success: true,
                summary: "read api.ts".into(),
            },
        );
        assert!(app.run.current.is_none());
        assert!(app.run.milestones.iter().any(|m| m.contains("read api.ts")));

        apply_event(
            &mut app,
            &AgentEvent::TextDelta {
                content: "Found a mismatch.".into(),
            },
        );
        apply_event(
            &mut app,
            &AgentEvent::TextDelta {
                content: " Will patch.".into(),
            },
        );
        match app.conversation.items.last() {
            Some(TimelineItem::Assistant(text)) => {
                assert!(text.contains("Found a mismatch"));
                assert!(text.contains("Will patch"));
            }
            other => panic!("expected assistant text, got {other:?}"),
        }
    }

    #[test]
    fn finished_emits_result_card_and_updates_context() {
        let mut app = empty_app();
        apply_event(
            &mut app,
            &AgentEvent::FileChanged {
                path: PathBuf::from("src/lib/api.ts"),
            },
        );
        apply_event(
            &mut app,
            &AgentEvent::VerificationCompleted {
                command: "eslint".into(),
                success: true,
                summary: "ok".into(),
            },
        );
        apply_event(
            &mut app,
            &AgentEvent::Finished {
                result: RunResult {
                    run_id: "r".into(),
                    status: RunStatus::Completed,
                    success: true,
                    summary: Some("Fixed the query param.".into()),
                    changed_files: vec![PathBuf::from("src/lib/api.ts")],
                    steps: 4,
                    model_turns: 3,
                    duration_ms: 23_000,
                    stop_reason: "TaskComplete".into(),
                    log_path: PathBuf::new(),
                    usage: RunUsage {
                        model_calls: 3,
                        input_tokens: 10_000,
                        output_tokens: 8_200,
                        cached_input_tokens: None,
                        estimated_cost_usd: None,
                    },
                },
            },
        );

        assert!(!app.run.busy);
        assert_eq!(app.sidebar.session.tokens, "18.2k");
        assert_eq!(app.sidebar.session.elapsed, "00:23");
        match app.conversation.items.last() {
            Some(TimelineItem::ResultCard { title, summary, .. }) => {
                assert!(title.contains("Verified"));
                assert!(summary.contains("query param"));
            }
            other => panic!("expected result card, got {other:?}"),
        }
    }
}
