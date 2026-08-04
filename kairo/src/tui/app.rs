//! Application state for the full-screen TUI.

use std::path::PathBuf;

use crate::api::ApprovalRequest;
use crate::tui::demo;
use crate::tui::feed::RunUiState;
use crate::tui::overlay::{ApprovalView, DiffView, Overlay};
use crate::tui::screens::{SessionBrowserState, WelcomeState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Welcome,
    Session,
    Sessions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarTab {
    Plan,
    Changes,
    Context,
}

impl SidebarTab {
    pub const ALL: [SidebarTab; 3] = [Self::Plan, Self::Changes, Self::Context];

    pub fn title(self) -> &'static str {
        match self {
            Self::Plan => "Plan",
            Self::Changes => "Changes",
            Self::Context => "Context",
        }
    }

    #[allow(dead_code)]
    pub fn shortcut(self) -> &'static str {
        match self {
            Self::Plan => "Ctrl+P",
            Self::Changes => "Ctrl+D",
            Self::Context => "Ctrl+I",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentModeUi {
    Ask,
    Plan,
    Agent,
}

impl AgentModeUi {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ask => "Ask",
            Self::Plan => "Plan",
            Self::Agent => "Agent",
        }
    }

    /// Toggle the live backend modes (Ask ↔ Agent). Plan is not a separate runtime mode.
    pub fn toggle_ask_agent(self) -> Self {
        match self {
            Self::Ask => Self::Agent,
            Self::Plan | Self::Agent => Self::Ask,
        }
    }
}

#[derive(Debug, Clone)]
pub struct HeaderState {
    pub brand: String,
    pub mode: AgentModeUi,
    pub model: String,
    pub workspace: String,
    pub edit_policy: String,
}

#[derive(Debug, Clone)]
pub enum TimelineItem {
    User(String),
    Assistant(String),
    Activity { title: String, lines: Vec<String> },
    ResultCard {
        title: String,
        summary: String,
        changed: Vec<String>,
        verification: Vec<String>,
        footer: String,
    },
}

#[derive(Debug, Clone)]
pub struct ConversationState {
    pub items: Vec<TimelineItem>,
    pub scroll: u16,
}

#[derive(Debug, Clone)]
pub struct PlanStepView {
    pub text: String,
    pub status: PlanStepStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanStepStatus {
    Done,
    Active,
    Todo,
}

#[derive(Debug, Clone)]
pub struct ChangeEntry {
    pub path: String,
    pub stats: String,
}

#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub mode: String,
    pub policy: String,
    pub model: String,
    pub workspace: String,
    pub session_id: String,
    pub tokens: String,
    pub elapsed: String,
}

#[derive(Debug, Clone)]
pub struct SidebarState {
    pub tab: SidebarTab,
    pub plan_done: usize,
    pub plan_total: usize,
    pub plan_steps: Vec<PlanStepView>,
    pub changes: Vec<ChangeEntry>,
    /// Selected row in the Changes tab.
    pub selected_change: usize,
    pub session: SessionInfo,
}

#[derive(Debug, Clone)]
pub struct ComposerState {
    pub text: String,
    pub placeholder: String,
    /// Visible widget height in rows (grows 3..=8 with content).
    pub height: u16,
    /// Cursor position in characters (not bytes).
    pub cursor: usize,
    /// Vertical scroll offset within the composer (logical lines).
    pub scroll: usize,
    pub history: Vec<String>,
    pub history_index: Option<usize>,
    /// Draft saved while browsing history.
    pub draft_backup: String,
}

#[derive(Debug, Clone)]
pub struct StatusState {
    pub hint: String,
}

#[derive(Debug, Clone)]
pub struct App {
    pub screen: Screen,
    pub header: HeaderState,
    pub conversation: ConversationState,
    pub sidebar: SidebarState,
    pub composer: ComposerState,
    pub status: StatusState,
    pub terminal_size: (u16, u16),
    pub run: RunUiState,
    pub overlay: Option<Overlay>,
    /// Absolute workspace path (for git diff lookups).
    pub workspace_path: PathBuf,
    pub welcome: WelcomeState,
    pub sessions: SessionBrowserState,
}

impl App {
    /// Static demo chrome (layout preview / tests).
    pub fn demo() -> Self {
        demo::sample_app()
    }

    /// Empty live session ready for agent events.
    pub fn live_session(
        model: &str,
        workspace_display: &str,
        mode_label: &str,
        policy_label: &str,
        session_id: &str,
        workspace_path: PathBuf,
    ) -> Self {
        let mode = match mode_label.to_ascii_lowercase().as_str() {
            "ask" => AgentModeUi::Ask,
            "plan" => AgentModeUi::Plan,
            _ => AgentModeUi::Agent,
        };
        let short_ws = workspace_display
            .rsplit('/')
            .next()
            .unwrap_or(workspace_display)
            .trim_start_matches('~')
            .trim_start_matches('/')
            .to_string();
        let short_ws = if short_ws.is_empty() {
            workspace_display.to_string()
        } else {
            short_ws
        };
        let short_id = if session_id.len() > 8 {
            format!("{}…", &session_id[..8])
        } else {
            session_id.to_string()
        };

        App {
            screen: Screen::Session,
            header: HeaderState {
                brand: "KAIRO".into(),
                mode,
                model: model.to_string(),
                workspace: workspace_display.to_string(),
                edit_policy: policy_label.to_string(),
            },
            conversation: ConversationState {
                items: vec![TimelineItem::Assistant(
                    "Session ready. Type a task and press Enter.".into(),
                )],
                scroll: 0,
            },
            sidebar: SidebarState {
                tab: SidebarTab::Plan,
                plan_done: 0,
                plan_total: 0,
                plan_steps: Vec::new(),
                changes: Vec::new(),
                selected_change: 0,
                session: SessionInfo {
                    mode: mode.as_str().to_string(),
                    policy: policy_label.to_string(),
                    model: model.to_string(),
                    workspace: short_ws,
                    session_id: short_id,
                    tokens: "—".into(),
                    elapsed: "—".into(),
                },
            },
            composer: ComposerState::new("Ask Kairo to explain, change, review, or test…"),
            status: StatusState {
                hint: "Enter send · Shift+Enter newline · ↑↓ history · Esc cancel · Ctrl+L clear · q quit"
                    .into(),
            },
            terminal_size: (120, 40),
            run: RunUiState::default(),
            overlay: None,
            workspace_path,
            welcome: WelcomeState::new(
                workspace_display,
                "—",
                "deepseek",
                model,
                policy_label,
            ),
            sessions: SessionBrowserState::from_list(Vec::new(), None),
        }
    }

    pub fn with_welcome(
        mut self,
        provider: &str,
        provider_id: &str,
        model: &str,
        workspace_display: &str,
        edit_policy: &str,
    ) -> Self {
        self.screen = Screen::Welcome;
        self.welcome = WelcomeState::new(
            workspace_display,
            provider,
            provider_id,
            model,
            edit_policy,
        );
        self.header.model = model.to_string();
        self.header.workspace = workspace_display.to_string();
        self.header.edit_policy = edit_policy.to_string();
        self.status.hint =
            "Enter start · s sessions · m change model · q quit".into();
        self
    }

    pub fn show_sessions(&mut self, entries: Vec<crate::session::SessionSummary>) {
        self.sessions =
            SessionBrowserState::from_list(entries, Some(self.workspace_path.clone()));
        self.screen = Screen::Sessions;
        self.status.hint =
            "↑↓ select · Enter resume · n new session · Esc back · q quit".into();
    }

    pub fn show_welcome(&mut self) {
        self.screen = Screen::Welcome;
        self.overlay = None;
        self.status.hint =
            "Enter start · s sessions · m change model · q quit".into();
    }

    pub fn open_model_picker(&mut self) {
        let provider_id = self.welcome.provider_id.clone();
        let mut models: Vec<String> = crate::user_config::curated_models(&provider_id)
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        if !models.iter().any(|m| m == &self.welcome.model) {
            models.insert(0, self.welcome.model.clone());
        }
        let selected = models
            .iter()
            .position(|m| m == &self.welcome.model)
            .unwrap_or(0);
        self.overlay = Some(Overlay::ModelPicker(crate::tui::overlay::ModelPickerView {
            provider_id,
            models,
            selected,
        }));
        self.status.hint = "↑↓ select model · Enter save · Esc cancel".into();
    }

    pub fn apply_selected_model(&mut self) -> Result<(), String> {
        let Some(Overlay::ModelPicker(picker)) = self.overlay.as_ref() else {
            return Ok(());
        };
        let model = picker
            .models
            .get(picker.selected)
            .cloned()
            .ok_or_else(|| "no model selected".to_string())?;
        let provider_id = picker.provider_id.clone();

        let mut cfg = crate::user_config::load()
            .map_err(|e| e.to_string())?
            .unwrap_or_else(|| crate::user_config::UserConfig {
                provider: provider_id.clone(),
                api_key: String::new(),
                model: model.clone(),
            });
        // Keep existing key; update model (+ provider id if missing).
        if cfg.provider.trim().is_empty() {
            cfg.provider = provider_id;
        }
        cfg.model = model.clone();
        if cfg.api_key.trim().is_empty() {
            // Prefer env key so save() succeeds when config was env-only.
            let env_key = crate::user_config::key_env_var(&cfg.provider);
            if let Ok(v) = std::env::var(env_key) {
                let v = v.trim().to_string();
                if !v.is_empty() {
                    cfg.api_key = v;
                }
            }
        }
        crate::user_config::save(&cfg).map_err(|e| e.to_string())?;

        self.welcome.model = model.clone();
        self.header.model = model.clone();
        self.sidebar.session.model = model.clone();
        self.welcome.message = Some(format!("Model set to `{model}` for the next session."));
        self.overlay = None;
        self.status.hint =
            "Enter start · s sessions · m change model · q quit".into();
        Ok(())
    }

    pub fn enter_session_screen(&mut self) {
        self.screen = Screen::Session;
        self.overlay = None;
        crate::tui::feed::refresh_status_hint(self);
    }

    /// Load prior messages / plan / files into the conversation panels.
    pub fn hydrate_from_persisted(&mut self, session: &crate::session::Session) {
        self.conversation.items.clear();
        for msg in &session.messages {
            match msg {
                crate::session::Message::User { content } => {
                    self.conversation
                        .items
                        .push(TimelineItem::User(content.clone()));
                }
                crate::session::Message::Assistant { content } => {
                    // Skip noisy session-update notes in the timeline.
                    if content.starts_with("Session update:") {
                        continue;
                    }
                    self.conversation
                        .items
                        .push(TimelineItem::Assistant(content.clone()));
                }
                _ => {}
            }
        }
        if self.conversation.items.is_empty() {
            self.conversation.items.push(TimelineItem::Assistant(
                "Resumed session. Type a task and press Enter.".into(),
            ));
        }

        if let Some(plan) = &session.task.plan {
            let mut views = Vec::new();
            let mut done = 0usize;
            let mut marked_active = false;
            for step in &plan.steps {
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
            self.sidebar.plan_steps = views;
            self.sidebar.plan_done = done;
            self.sidebar.plan_total = plan.steps.len();
        }

        self.sidebar.changes = session
            .task
            .modified_files
            .iter()
            .map(|p| ChangeEntry {
                path: p.display().to_string(),
                stats: "changed".into(),
            })
            .collect();
        self.sidebar.selected_change = 0;
    }

    pub fn set_sidebar_tab(&mut self, tab: SidebarTab) {
        self.sidebar.tab = tab;
    }

    pub fn cycle_mode(&mut self) {
        self.header.mode = self.header.mode.toggle_ask_agent();
        self.sidebar.session.mode = self.header.mode.as_str().to_string();
        match self.header.mode {
            AgentModeUi::Ask => {
                self.header.edit_policy = "Read-only".into();
                self.sidebar.session.policy = "Read-only".into();
            }
            AgentModeUi::Agent | AgentModeUi::Plan => {
                self.header.edit_policy = "Confirm edits".into();
                self.sidebar.session.policy = "Confirm".into();
            }
        }
    }

    pub fn api_mode(&self) -> crate::api::AgentMode {
        match self.header.mode {
            AgentModeUi::Ask => crate::api::AgentMode::Ask,
            AgentModeUi::Plan | AgentModeUi::Agent => crate::api::AgentMode::Agent,
        }
    }

    /// Clear the conversation timeline (keeps sidebar / session metadata).
    pub fn clear_conversation(&mut self) {
        self.conversation.items.clear();
        self.conversation.scroll = 0;
        self.run.milestones.clear();
        self.run.current = None;
        self.run.verification.clear();
    }

    pub fn show_approval(&mut self, request: ApprovalRequest) {
        self.run.pending_approval = Some(request.clone());
        self.overlay = Some(Overlay::Approval(ApprovalView { request }));
        crate::tui::feed::refresh_status_hint(self);
    }

    pub fn clear_approval_overlay(&mut self) {
        self.run.pending_approval = None;
        match self.overlay.take() {
            Some(Overlay::Approval(_)) => {}
            Some(Overlay::Diff(view)) if view.return_to_approval.is_some() => {}
            other => self.overlay = other,
        }
    }

    pub fn open_diff_for_selected_change(&mut self) {
        let Some(entry) = self.sidebar.changes.get(self.sidebar.selected_change) else {
            return;
        };
        let path = entry.path.clone();
        let body = crate::tui::overlay::load_git_diff(&self.workspace_path, &path)
            .unwrap_or_else(|| {
                format!(
                    "No git diff for `{path}` yet.\n\
                     The file is listed as changed; open again after the edit lands,\n\
                     or approve a pending edit to apply it."
                )
            });
        self.overlay = Some(Overlay::Diff(DiffView::from_text(
            format!("Diff: {path}"),
            &body,
        )));
        crate::tui::feed::refresh_status_hint(self);
    }

    pub fn open_diff_for_approval(&mut self) {
        let Some(request) = self.run.pending_approval.clone() else {
            return;
        };
        let path = request
            .paths
            .first()
            .map(|p| p.display().to_string())
            .unwrap_or_default();

        // Prefer the proposed patch preview; fall back to git diff, then metadata.
        let mut view = if let Some(preview) = request
            .preview
            .as_deref()
            .filter(|s| !s.trim().is_empty())
        {
            let title = if path.is_empty() {
                "Proposed diff".into()
            } else {
                format!("Diff: {path}")
            };
            DiffView::from_text(title, preview)
        } else if !path.is_empty() {
            if let Some(body) = crate::tui::overlay::load_git_diff(&self.workspace_path, &path) {
                DiffView::from_text(format!("Diff: {path}"), &body)
            } else {
                DiffView::from_approval_fallback(&request)
            }
        } else {
            DiffView::from_approval_fallback(&request)
        };
        view.return_to_approval = Some(request);
        self.overlay = Some(Overlay::Diff(view));
        crate::tui::feed::refresh_status_hint(self);
    }

    pub fn close_overlay(&mut self) {
        match self.overlay.take() {
            Some(Overlay::Diff(view)) => {
                if let Some(request) = view.return_to_approval {
                    self.overlay = Some(Overlay::Approval(ApprovalView { request }));
                }
            }
            Some(Overlay::Approval(_)) => {
                self.overlay = None;
            }
            Some(Overlay::Help) | Some(Overlay::ModelPicker(_)) | None => {}
        }
        crate::tui::feed::refresh_status_hint(self);
    }

    pub fn select_change_delta(&mut self, delta: i32) {
        if self.sidebar.changes.is_empty() {
            return;
        }
        let len = self.sidebar.changes.len() as i32;
        let cur = self.sidebar.selected_change as i32;
        let next = (cur + delta).rem_euclid(len);
        self.sidebar.selected_change = next as usize;
    }
}
