use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::agent::{AgentRuntime, RunResult};
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
                println!("[agent-v1] EOF — saving session");
                break;
            };

            match parse_input(&line) {
                ChatInput::Exit => {
                    println!("[agent-v1] exiting");
                    break;
                }
                ChatInput::Empty => continue,
                ChatInput::Message(message) => {
                    if let Err(e) = self.handle_message(&message).await {
                        eprintln!("[agent-v1] error: {e:#}");
                    }
                }
            }
        }

        self.manager
            .save(&self.session)
            .context("failed to save session on exit")?;
        println!("[agent-v1] session saved: {}", self.session.id);

        Ok(())
    }

    async fn handle_message(&mut self, message: &str) -> Result<()> {
        let result = self
            .agent
            .run(
                message,
                self.workspace.clone(),
                None,
                Some(&mut self.session),
            )
            .await?;

        self.manager
            .save(&self.session)
            .context("failed to save session after run")?;

        self.print_run_summary(&result);
        Ok(())
    }

    fn print_banner(&self) {
        println!("[agent-v1] interactive session: {}", self.session.id);
        println!("[agent-v1] workspace: {}", self.workspace.display());
        println!("[agent-v1] type a task, or /exit to quit");
        if !self.session.messages.is_empty() {
            println!(
                "[agent-v1] {} prior message(s) in session",
                self.session.messages.len()
            );
        }
    }

    fn print_run_summary(&self, result: &RunResult) {
        println!("\n[agent-v1] ── run complete ──────────────────────────");
        println!("[agent-v1] run_id: {}", result.run_id);
        println!("[agent-v1] stop reason: {:?}", result.stop_reason);
        println!("[agent-v1] total steps: {}", result.total_steps);
        if let Some(summary) = &result.summary {
            println!("[agent-v1] summary: {summary}");
        }
        println!("[agent-v1] log: {}", result.log_path.display());
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
            println!("[agent-v1] new session: {}", session.id);
            Ok(session)
        }
        SessionArg::Load(id) => {
            let session = manager.load(&id)?;
            println!(
                "[agent-v1] resumed session: {} ({} prior messages)",
                session.id,
                session.messages.len()
            );
            if session.workspace_root != workspace_root {
                eprintln!(
                    "[agent-v1] warning: session workspace ({}) differs from config ({})",
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
    use crate::llm::{LlmClient, LlmError};
    use crate::state::step::Action;

    struct FakeLlm;

    #[async_trait]
    impl LlmClient for FakeLlm {
        async fn next_action(&self, _request: &LlmRequest) -> Result<Action, LlmError> {
            Ok(Action::Finish {
                summary: "done".to_string(),
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

        let agent = AgentRuntime::new(Box::new(FakeLlm), "fake", 5, 0, 0, 0, "system");
        let mut chat = ChatRuntime::new(manager, session, agent, workspace);

        chat.handle_message("fix bug").await.unwrap();

        assert_eq!(chat.session.messages.len(), 2);
        assert_eq!(
            chat.session.messages[0],
            crate::session::Message::user("fix bug")
        );
    }
}
