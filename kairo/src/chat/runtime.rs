use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::agent::{AgentRuntime, RunResult};
use crate::authorization::StdinApprovalGate;
use crate::session::{Session, SessionManager, SessionSettings};

use super::commands::{parse_input, ChatInput};

/// How the CLI selects or creates a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionArg {
    New,
    Load(String),
}

/// Manages an interactive agent session (stdin → agent → stdout).
pub struct ChatRuntime {
    manager: SessionManager,
    session: Session,
    agent: AgentRuntime,
    workspace: PathBuf,
}

impl ChatRuntime {
    pub fn new(
        manager: SessionManager,
        session: Session,
        agent: AgentRuntime,
        workspace: PathBuf,
    ) -> Self {
        Self {
            manager,
            session,
            agent,
            workspace,
        }
    }

    pub fn session_id(&self) -> &str {
        &self.session.id
    }

    pub async fn run_loop(&mut self) -> Result<()> {
        self.print_banner();

        loop {
            print!("\n> ");
            io::stdout().flush().ok();

            let line = read_line().await?;
            let Some(line) = line else {
                println!("[kairo] EOF — saving session");
                break;
            };

            match parse_input(&line) {
                ChatInput::Exit => {
                    println!("[kairo] exiting");
                    break;
                }
                ChatInput::Empty => continue,
                ChatInput::ShowPlan => self.print_plan(),
                ChatInput::ShowTask => self.print_task(),
                ChatInput::Overview { depth, focus } => {
                    if let Err(e) = self.print_overview(&depth, focus.as_deref()) {
                        eprintln!("[kairo] {e:#}");
                    }
                }
                ChatInput::ApprovePlan => {
                    if let Err(e) = self.approve_plan() {
                        eprintln!("[kairo] {e}");
                    }
                }
                ChatInput::ConstrainPlan(text) => {
                    if let Err(e) = self.constrain_plan(&text) {
                        eprintln!("[kairo] {e}");
                    }
                }
                ChatInput::RecordDecision(text) => {
                    if let Err(e) = self.record_decision(&text) {
                        eprintln!("[kairo] {e}");
                    }
                }
                ChatInput::SetIntent(intent) => {
                    if let Err(e) = self.set_intent(intent) {
                        eprintln!("[kairo] {e}");
                    }
                }
                ChatInput::ClearIntent => {
                    if let Err(e) = self.clear_intent() {
                        eprintln!("[kairo] {e}");
                    }
                }
                ChatInput::SetEditPolicy(policy) => {
                    if let Err(e) = self.set_edit_policy(policy) {
                        eprintln!("[kairo] {e}");
                    }
                }
                ChatInput::Message(message) => {
                    if let Err(e) = self.handle_message(&message).await {
                        eprintln!("[kairo] error: {e:#}");
                    }
                }
            }
        }

        self.manager
            .save(&self.session)
            .context("failed to save session on exit")?;
        println!("[kairo] session saved: {}", self.session.id);

        Ok(())
    }

    async fn handle_message(&mut self, message: &str) -> Result<()> {
        let gate = StdinApprovalGate;
        let result = self
            .agent
            .run(
                message,
                self.workspace.clone(),
                None,
                Some(&mut self.session),
                Some(&gate),
            )
            .await?;

        self.manager
            .save(&self.session)
            .context("failed to save session after run")?;

        self.print_run_summary(&result);
        Ok(())
    }

    fn print_banner(&self) {
        println!("[kairo] interactive session: {}", self.session.id);
        println!("[kairo] workspace: {}", self.workspace.display());
        println!(
            "[kairo] edit_policy: {:?}{}",
            self.session.authorization.edit_policy,
            if self.session.authorization.workspace_mutations_approved {
                " (workspace mutations approved for task)"
            } else {
                ""
            }
        );
        println!(
            "[kairo] turn_intent: {}{}",
            self.session.declared_intent.as_str(),
            if self.session.intent_pinned {
                " (pinned)"
            } else {
                " (auto)"
            }
        );
        println!("[kairo] type a task, or /exit to quit");
        println!(
            "[kairo] cmds: /overview  /intent <explore|plan|execute|review|clear>  /edit-policy <auto|confirm|readonly>  /plan  /approve-plan  /constrain <text>  /decision <text>  /task  /exit"
        );
        if !self.session.messages.is_empty() {
            println!(
                "[kairo] {} prior message(s) in session",
                self.session.messages.len()
            );
        }
        if self.session.task.plan.is_some() {
            println!("[kairo] session has a durable plan (/plan to view)");
        }
    }

    fn print_plan(&self) {
        match &self.session.task.plan {
            Some(plan) => {
                println!("\n[kairo] ── session plan ─────────────────────────");
                println!("{}", plan.render());
            }
            None => println!("[kairo] no plan in this session yet"),
        }
    }

    fn print_task(&self) {
        println!("\n[kairo] ── task state ──────────────────────────");
        print!("{}", self.session.task.render_compact());
    }

    fn print_overview(&self, depth: &str, focus: Option<&str>) -> Result<()> {
        use crate::repo_intel::{
            summarize, HybridIntel, SummaryBudget, SummaryDepth, SummaryRequest, SummaryScope,
        };
        use crate::tools::project_snapshot;

        let depth = SummaryDepth::parse(depth).unwrap_or(SummaryDepth::Overview);
        let (intel, stats) = HybridIntel::build(
            &self.workspace,
            self.workspace.display().to_string(),
        )
        .map_err(|e| anyhow::anyhow!("index build failed: {e}"))?;
        let snapshot = project_snapshot::detect(&self.workspace);
        let request = SummaryRequest {
            scope: SummaryScope::Repository,
            depth,
            budget: SummaryBudget::for_depth(depth),
            focus: focus.map(|s| s.to_string()),
        };
        let summary = summarize(
            &self.workspace,
            intel.index(),
            Some(&snapshot),
            &request,
        );
        println!("\n[kairo] ── repository overview ───────────────");
        println!(
            "[kairo] index: {} file(s) in {}ms",
            stats.files_indexed, stats.build_ms
        );
        println!("{}", summary.text);
        Ok(())
    }

    fn approve_plan(&mut self) -> Result<()> {
        let version = {
            let Some(plan) = self.session.task.plan.as_mut() else {
                anyhow::bail!("no plan to approve — ask the agent to update_plan first");
            };
            plan.approve();
            plan.version
        };
        let _ = self
            .session
            .task
            .record_decision(format!("approved plan v{version}"));
        self.session
            .append_session_update(format!("Plan v{version} approved."));
        self.manager.save(&self.session)?;
        println!("[kairo] plan approved (v{version})");
        if let Some(plan) = &self.session.task.plan {
            println!("{}", plan.render());
        }
        Ok(())
    }

    fn constrain_plan(&mut self, text: &str) -> Result<()> {
        let version = {
            let Some(plan) = self.session.task.plan.as_mut() else {
                anyhow::bail!("no plan to constrain — ask the agent to update_plan first");
            };
            plan.add_constraint(text)
                .map_err(anyhow::Error::msg)?;
            plan.version
        };
        let _ = self
            .session
            .task
            .record_decision(format!("constraint: {text}"));
        self.session
            .append_session_update(format!("Constraint added (plan v{version}): {text}"));
        self.manager.save(&self.session)?;
        println!("[kairo] constraint added; plan back to draft (v{version})");
        if let Some(plan) = &self.session.task.plan {
            println!("{}", plan.render());
        }
        Ok(())
    }

    fn record_decision(&mut self, text: &str) -> Result<()> {
        self.session
            .task
            .record_decision(text)
            .map_err(anyhow::Error::msg)?;
        self.session
            .append_session_update(format!("Decision recorded: {text}"));
        self.manager.save(&self.session)?;
        println!("[kairo] decision recorded: {text}");
        Ok(())
    }

    fn set_intent(&mut self, intent: crate::session::TurnIntent) -> Result<()> {
        self.session.set_intent(intent);
        self.manager.save(&self.session)?;
        println!(
            "[kairo] turn_intent pinned to {} (metadata only — EditPolicy still governs edits)",
            intent.as_str()
        );
        Ok(())
    }

    fn clear_intent(&mut self) -> Result<()> {
        self.session.clear_intent_pin();
        self.manager.save(&self.session)?;
        println!("[kairo] turn_intent pin cleared — will infer from messages");
        Ok(())
    }

    fn set_edit_policy(&mut self, policy: crate::authorization::EditPolicy) -> Result<()> {
        self.session.set_edit_policy(policy);
        self.manager.save(&self.session)?;
        println!(
            "[kairo] edit_policy set to {} (appended session update)",
            policy.as_str()
        );
        Ok(())
    }

    fn print_run_summary(&self, result: &RunResult) {
        println!("\n[kairo] ── run complete ──────────────────────────");
        println!("[kairo] run_id: {}", result.run_id);
        println!("[kairo] stop reason: {:?}", result.stop_reason);
        println!("[kairo] {}", result.efficiency.render_compact());
        if let Some(summary) = &result.summary {
            println!("[kairo] summary: {summary}");
        }
        println!("[kairo] log: {}", result.log_path.display());
        println!();
        for line in result.usage.render_report().lines() {
            println!("[kairo] {line}");
        }
    }
}

pub fn open_session(
    manager: &SessionManager,
    session_arg: SessionArg,
    workspace_root: &Path,
    settings: SessionSettings,
) -> Result<Session> {
    match session_arg {
        SessionArg::New => {
            let session = manager.create(workspace_root, settings)?;
            println!("[kairo] new session: {}", session.id);
            Ok(session)
        }
        SessionArg::Load(id) => {
            let session = manager.load(&id)?;
            println!(
                "[kairo] resumed session: {} ({} prior messages)",
                session.id,
                session.messages.len()
            );
            if session.workspace_root != workspace_root {
                eprintln!(
                    "[kairo] warning: session workspace ({}) differs from config ({})",
                    session.workspace_root.display(),
                    workspace_root.display()
                );
            }
            Ok(session)
        }
    }
}

async fn read_line() -> Result<Option<String>> {
    tokio::task::spawn_blocking(|| {
        let mut line = String::new();
        let bytes = io::stdin().read_line(&mut line)?;
        if bytes == 0 {
            return Ok(None);
        }
        Ok(Some(line))
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use tempfile::tempdir;

    use crate::context::LlmRequest;
    use crate::llm::{AgentResponse, LlmClient, LlmError, LlmToolCall, LlmTurn};
    use crate::state::step::Action;

    struct FakeLlm;

    #[async_trait]
    impl LlmClient for FakeLlm {
        async fn respond(&self, _request: &LlmRequest) -> Result<LlmTurn, LlmError> {
            Ok(LlmTurn {
                response: AgentResponse::ToolCalls(vec![LlmToolCall {
                    tool_call_id: "call_fake_finish".to_string(),
                    action: Action::Finish {
                        summary: "done".to_string(),
                    },
                }]),
                usage: crate::llm::ModelUsage::default(),
            })
        }

        fn model_id(&self) -> &str {
            "fake"
        }
    }

    #[tokio::test]
    async fn handle_message_updates_session() {
        let dir = tempdir().unwrap();
        let workspace = dir.path().to_path_buf();
        let manager = SessionManager::new(dir.path().join("sessions"));
        let session = manager
            .create(&workspace, SessionSettings::default())
            .unwrap();

        let agent = AgentRuntime::new(Box::new(FakeLlm), "fake", 5, 0, 0, 0, "system", 16000, 2048, 20);
        let mut chat = ChatRuntime::new(manager, session, agent, workspace);

        chat.handle_message("fix bug").await.unwrap();

        assert_eq!(chat.session.messages.len(), 2);
        assert_eq!(
            chat.session.messages[0],
            crate::session::Message::user("fix bug")
        );
    }
}
