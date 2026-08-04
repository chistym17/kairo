//! Static sample content used to verify the Part 1 shell.

use super::app::{
    AgentModeUi, App, ChangeEntry, ComposerState, ConversationState, HeaderState, PlanStepStatus,
    PlanStepView, Screen, SessionInfo, SidebarState, SidebarTab, StatusState, TimelineItem,
};
use super::feed::RunUiState;
use super::screens::{SessionBrowserState, WelcomeState};

pub fn sample_app() -> App {
    App {
        screen: Screen::Session,
        header: HeaderState {
            brand: "KAIRO".into(),
            mode: AgentModeUi::Agent,
            model: "deepseek-v4-flash".into(),
            workspace: "~/projects/annotaa".into(),
            edit_policy: "Confirm edits".into(),
        },
        conversation: ConversationState {
            items: vec![
                TimelineItem::User("Fix the date filtering bug.".into()),
                TimelineItem::Assistant(
                    "I found that the frontend sends `due_date`, while the backend expects `date`."
                        .into(),
                ),
                TimelineItem::Activity {
                    title: "Activity".into(),
                    lines: vec![
                        "✓ Read frontend/src/lib/api.ts".into(),
                        "✓ Read backend/tasks/views.py".into(),
                        "⠋ Applying patch to src/lib/api.ts".into(),
                    ],
                },
                TimelineItem::ResultCard {
                    title: "Completed — Verified".into(),
                    summary: "Updated the task request to use the query parameter expected by the backend."
                        .into(),
                    changed: vec!["M frontend/src/lib/api.ts                     +1 -1".into()],
                    verification: vec!["✓ npx eslint src/lib/api.ts".into()],
                    footer: "1 file changed · 11 turns · 18.2k tokens".into(),
                },
            ],
            scroll: 0,
        },
        sidebar: SidebarState {
            tab: SidebarTab::Plan,
            plan_done: 2,
            plan_total: 4,
            plan_steps: vec![
                PlanStepView {
                    text: "Inspect task loading".into(),
                    status: PlanStepStatus::Done,
                },
                PlanStepView {
                    text: "Compare API contract".into(),
                    status: PlanStepStatus::Done,
                },
                PlanStepView {
                    text: "Patch client request".into(),
                    status: PlanStepStatus::Active,
                },
                PlanStepView {
                    text: "Verify changed file".into(),
                    status: PlanStepStatus::Todo,
                },
            ],
            changes: vec![ChangeEntry {
                path: "src/lib/api.ts".into(),
                stats: "+1 -1".into(),
            }],
            selected_change: 0,
            session: SessionInfo {
                mode: "Agent".into(),
                policy: "Confirm".into(),
                model: "DeepSeek V4 Flash".into(),
                workspace: "annotaa".into(),
                session_id: "7dbf…".into(),
                tokens: "18.2k".into(),
                elapsed: "00:23".into(),
            },
        },
        composer: ComposerState::new("Ask Kairo to explain, change, review, or test…"),
        status: StatusState {
            hint: "Enter send · Shift+Enter newline · ↑↓ history · Esc cancel · Ctrl+L clear · q quit · ? help"
                .into(),
        },
        terminal_size: (120, 40),
        run: RunUiState {
            busy: true,
            current: Some("Applying patch to src/lib/api.ts".into()),
            milestones: vec![
                "✓ Read frontend/src/lib/api.ts".into(),
                "✓ Read backend/tasks/views.py".into(),
            ],
            verification: Vec::new(),
            pending_approval: None,
            spinner_frame: 0,
            started_at: None,
        },
        overlay: None,
        workspace_path: std::path::PathBuf::from("."),
        welcome: WelcomeState::new(
            "~/projects/annotaa",
            "DeepSeek",
            "deepseek-v4-flash",
            "Confirm edits",
        ),
        sessions: SessionBrowserState::from_list(Vec::new(), None),
    }
}
