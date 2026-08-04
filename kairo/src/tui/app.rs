//! Application state for the full-screen TUI.

use crate::tui::demo;
use crate::tui::feed::RunUiState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // Welcome / Sessions screens land in a later part.
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

    pub fn next(self) -> Self {
        match self {
            Self::Ask => Self::Plan,
            Self::Plan => Self::Agent,
            Self::Agent => Self::Ask,
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
    #[allow(dead_code)] // Multi-screen navigation lands in a later part.
    pub screen: Screen,
    pub header: HeaderState,
    pub conversation: ConversationState,
    pub sidebar: SidebarState,
    pub composer: ComposerState,
    pub status: StatusState,
    pub terminal_size: (u16, u16),
    pub run: RunUiState,
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
        }
    }

    pub fn set_sidebar_tab(&mut self, tab: SidebarTab) {
        self.sidebar.tab = tab;
    }

    pub fn cycle_mode(&mut self) {
        // Visual-only for now; live AgentMode is fixed for the open session.
        self.header.mode = self.header.mode.next();
        self.sidebar.session.mode = self.header.mode.as_str().to_string();
    }

    /// Clear the conversation timeline (keeps sidebar / session metadata).
    pub fn clear_conversation(&mut self) {
        self.conversation.items.clear();
        self.conversation.scroll = 0;
        self.run.milestones.clear();
        self.run.current = None;
        self.run.verification.clear();
    }
}
