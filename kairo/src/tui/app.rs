//! Application state for the full-screen TUI.

use crate::tui::demo;

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
    /// done | active | todo
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
    /// Visible height in rows (grows 3..=8 later; fixed for Part 1).
    pub height: u16,
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
}

impl App {
    pub fn demo() -> Self {
        demo::sample_app()
    }

    pub fn set_sidebar_tab(&mut self, tab: SidebarTab) {
        self.sidebar.tab = tab;
    }

    pub fn cycle_mode(&mut self) {
        self.header.mode = self.header.mode.next();
        self.sidebar.session.mode = self.header.mode.as_str().to_string();
        self.status.hint = format!(
            "Tab mode   {} plan   {} diff   {} context   q quit",
            SidebarTab::Plan.shortcut(),
            SidebarTab::Changes.shortcut(),
            SidebarTab::Context.shortcut(),
        );
    }
}
