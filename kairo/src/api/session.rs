use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

use crate::agent::{AgentEvent as InternalEvent, AgentObserver, AgentRuntime};
use crate::authorization::{
    ApprovalDecision, ApprovalGate, ApprovalRequest as InternalApprovalRequest,
};
use crate::llm::{build_client, LlmClient};
use crate::session::{Session, SessionManager, SessionSettings};

use super::config::{AgentConfig, AgentMode, EditPolicy, SessionSource};
use super::events::{
    map_approval_request, map_event, map_run_result, AgentEvent, RunResult, RunStatus,
};

/// A live agent attached to one workspace.
///
/// The run executes on its own thread, so [`AgentSession`] can be used from any
/// async runtime (or from synchronous code). Task memory — messages, plan,
/// decisions, verification history — persists across [`AgentSession::send`] calls.
pub struct AgentSession {
    session_id: String,
    mode: AgentMode,
    edit_policy: EditPolicy,
    workspace: PathBuf,
    commands: Sender<Command>,
    decisions: Sender<DecisionMessage>,
    events: Mutex<Option<EventStream>>,
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
    cancel: Arc<AtomicBool>,
}

enum Command {
    Send(String),
    Shutdown,
}

struct DecisionMessage {
    approval_id: String,
    decision: ApprovalDecision,
}

/// Receiver for [`AgentEvent`]s of a session.
#[derive(Debug)]
pub struct EventStream {
    rx: UnboundedReceiver<AgentEvent>,
}

impl EventStream {
    /// Next event, or `None` once the session has shut down.
    pub async fn recv(&mut self) -> Option<AgentEvent> {
        self.rx.recv().await
    }

    /// Next buffered event without waiting.
    pub fn try_recv(&mut self) -> Option<AgentEvent> {
        self.rx.try_recv().ok()
    }
}

impl AgentSession {
    /// Open a session using the provider configured in [`AgentConfig`].
    pub fn open(config: AgentConfig) -> Result<Self> {
        let llm = build_client(&config.inner);
        Self::open_with_client(config, llm)
    }

    /// Open a session against a caller-supplied model client (tests, custom providers).
    pub fn open_with_client(config: AgentConfig, llm: Box<dyn LlmClient>) -> Result<Self> {
        let settings = SessionSettings {
            model_id: config.inner.model_id.clone(),
            max_steps: config.inner.max_steps,
            edit_policy: config.edit_policy.into(),
        };

        let (manager, mut session) = match &config.session_source {
            SessionSource::Ephemeral => (None, Session::new(config.workspace.clone(), settings)),
            SessionSource::Persistent => {
                let manager = SessionManager::with_default_dir();
                let session = manager
                    .create(config.workspace.clone(), settings)
                    .context("failed to create session")?;
                (Some(manager), session)
            }
            SessionSource::Resume(id) => {
                let manager = SessionManager::with_default_dir();
                let mut session = manager
                    .load(id)
                    .with_context(|| format!("failed to load session {id}"))?;
                session.settings = settings;
                (Some(manager), session)
            }
        };
        // The requested policy wins over whatever a resumed session recorded.
        session.authorization.edit_policy = config.edit_policy.into();
        session.authorization.revoke_task_approvals();

        let runtime = AgentRuntime::new(
            llm,
            &config.inner.model_id,
            config.inner.max_steps,
            config.inner.step_delay_ms,
            config.inner.tool_use_max_retries,
            config.inner.tool_use_retry_delay_ms,
            config.system_prompt.clone(),
            config.inner.max_context_tokens,
            config.inner.context_reserve_tokens,
            config.inner.keep_recent_session_messages,
        )
        .with_semantic_policy(config.inner.semantic_policy);

        let (event_tx, event_rx) = unbounded_channel();
        let (command_tx, command_rx) = std::sync::mpsc::channel();
        let (decision_tx, decision_rx) = std::sync::mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));

        let session_id = session.id.clone();
        let worker = Worker {
            runtime,
            session,
            manager,
            workspace: config.workspace.clone(),
            mode: config.mode,
            events: event_tx.clone(),
            cancel: Arc::clone(&cancel),
            gate: ChannelGate {
                events: event_tx,
                decisions: Mutex::new(decision_rx),
                timeout: config.approval_timeout,
                cancel: Arc::clone(&cancel),
            },
        };

        let handle = std::thread::Builder::new()
            .name(format!("agent-session-{session_id}"))
            .spawn(move || worker.run(command_rx))
            .context("failed to start agent session thread")?;

        Ok(Self {
            session_id,
            mode: config.mode,
            edit_policy: config.edit_policy,
            workspace: config.workspace,
            commands: command_tx,
            decisions: decision_tx,
            events: Mutex::new(Some(EventStream { rx: event_rx })),
            worker: Mutex::new(Some(handle)),
            cancel,
        })
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn mode(&self) -> AgentMode {
        self.mode
    }

    pub fn edit_policy(&self) -> EditPolicy {
        self.edit_policy
    }

    pub fn workspace(&self) -> &std::path::Path {
        &self.workspace
    }

    /// Take the event stream. Available once per session (single consumer).
    pub fn subscribe(&self) -> Result<EventStream> {
        self.events
            .lock()
            .expect("event stream mutex poisoned")
            .take()
            .ok_or_else(|| anyhow!("event stream already taken: subscribe() is single-consumer"))
    }

    /// Queue a task or question. Returns as soon as the run is queued; follow
    /// progress on the event stream, which ends the turn with
    /// [`AgentEvent::Finished`].
    pub async fn send(&self, message: impl Into<String>) -> Result<()> {
        let message = message.into();
        if message.trim().is_empty() {
            bail!("message is empty");
        }
        self.commands
            .send(Command::Send(message))
            .map_err(|_| anyhow!("session is no longer running"))
    }

    /// Allow the pending action once.
    pub async fn approve(&self, approval_id: impl Into<String>) -> Result<()> {
        self.decide(approval_id, ApprovalDecision::ApproveOnce)
    }

    /// Allow the pending edit and every later workspace edit in this task.
    /// External side effects still ask each time.
    pub async fn approve_for_task(&self, approval_id: impl Into<String>) -> Result<()> {
        self.decide(approval_id, ApprovalDecision::ApproveForTask)
    }

    /// Deny the pending action. The agent is told and continues.
    pub async fn reject(&self, approval_id: impl Into<String>) -> Result<()> {
        self.decide(approval_id, ApprovalDecision::Reject)
    }

    /// Ask the running turn to stop after the current model/tool step.
    /// Also rejects a pending approval wait so the loop can exit.
    pub fn request_stop(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    /// Whether a cooperative stop has been requested for the current turn.
    pub fn stop_requested(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    fn decide(&self, approval_id: impl Into<String>, decision: ApprovalDecision) -> Result<()> {
        self.decisions
            .send(DecisionMessage {
                approval_id: approval_id.into(),
                decision,
            })
            .map_err(|_| anyhow!("session is no longer running"))
    }

    /// Finish any queued task, persist the session, and stop the worker thread.
    pub fn shutdown(&self) -> Result<()> {
        let _ = self.commands.send(Command::Shutdown);
        if let Some(handle) = self
            .worker
            .lock()
            .expect("worker mutex poisoned")
            .take()
        {
            handle
                .join()
                .map_err(|_| anyhow!("agent session thread panicked"))?;
        }
        Ok(())
    }
}

impl Drop for AgentSession {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
        if let Some(handle) = self.worker.lock().ok().and_then(|mut w| w.take()) {
            let _ = handle.join();
        }
    }
}

/// Owns the agent and drives one task at a time on a dedicated thread.
struct Worker {
    runtime: AgentRuntime,
    session: Session,
    manager: Option<SessionManager>,
    workspace: PathBuf,
    mode: AgentMode,
    events: UnboundedSender<AgentEvent>,
    gate: ChannelGate,
    cancel: Arc<AtomicBool>,
}

impl Worker {
    fn run(mut self, commands: Receiver<Command>) {
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                let _ = self.events.send(AgentEvent::Error {
                    message: format!("failed to start session runtime: {e}"),
                });
                return;
            }
        };

        while let Ok(command) = commands.recv() {
            match command {
                Command::Send(message) => runtime.block_on(self.run_task(message)),
                Command::Shutdown => break,
            }
        }

        if let Some(manager) = &self.manager {
            if let Err(e) = manager.save(&self.session) {
                let _ = self.events.send(AgentEvent::Error {
                    message: format!("failed to save session: {e:#}"),
                });
            }
        }
    }

    async fn run_task(&mut self, message: String) {
        self.cancel.store(false, Ordering::SeqCst);
        let observer = ChannelObserver {
            events: self.events.clone(),
            mode: self.mode,
        };

        let outcome = self
            .runtime
            .run_with_cancel(
                message,
                self.workspace.clone(),
                Some(&observer),
                Some(&mut self.session),
                Some(&self.gate),
                Some(Arc::clone(&self.cancel)),
            )
            .await;

        let mut result = match outcome {
            Ok(result) => map_run_result(result),
            Err(e) => {
                let message = format!("{e:#}");
                let _ = self.events.send(AgentEvent::Error {
                    message: message.clone(),
                });
                RunResult::stopped(message)
            }
        };

        // Ask mode cannot verify anything, so an answered question is complete.
        if self.mode == AgentMode::Ask && matches!(result.status, RunStatus::CompletedUnverified) {
            result.status = RunStatus::Completed;
        }

        if let Some(manager) = &self.manager {
            if let Err(e) = manager.save(&self.session) {
                let _ = self.events.send(AgentEvent::Error {
                    message: format!("failed to save session: {e:#}"),
                });
            }
        }

        let _ = self.events.send(AgentEvent::Finished { result });
    }
}

/// Forwards internal observer events onto the public stream.
struct ChannelObserver {
    events: UnboundedSender<AgentEvent>,
    mode: AgentMode,
}

impl AgentObserver for ChannelObserver {
    fn on_event(&self, event: InternalEvent) {
        let mapped = match event {
            InternalEvent::RunStarted {
                run_id,
                workspace_root,
                model_id,
                ..
            } => Some(AgentEvent::Started {
                run_id,
                mode: self.mode.as_str().to_string(),
                workspace: PathBuf::from(workspace_root),
                model: model_id,
            }),
            other => map_event(other),
        };
        if let Some(event) = mapped {
            let _ = self.events.send(event);
        }
    }
}

/// Turns the runtime's blocking approval call into an event plus a decision wait.
struct ChannelGate {
    events: UnboundedSender<AgentEvent>,
    decisions: Mutex<Receiver<DecisionMessage>>,
    timeout: Duration,
    cancel: Arc<AtomicBool>,
}

impl ApprovalGate for ChannelGate {
    fn request_approval(&self, request: &InternalApprovalRequest) -> ApprovalDecision {
        let public = map_approval_request(request);
        let approval_id = public.id.clone();
        if self
            .events
            .send(AgentEvent::ApprovalRequired { request: public })
            .is_err()
        {
            return ApprovalDecision::Reject;
        }

        let decisions = self
            .decisions
            .lock()
            .expect("approval decision mutex poisoned");

        let deadline = Instant::now() + self.timeout;
        loop {
            if self.cancel.load(Ordering::Relaxed) {
                let _ = self.events.send(AgentEvent::Error {
                    message: format!("approval {approval_id} rejected — stop requested"),
                });
                return ApprovalDecision::Reject;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                let _ = self.events.send(AgentEvent::Error {
                    message: format!(
                        "approval {approval_id} timed out after {}s — rejected",
                        self.timeout.as_secs()
                    ),
                });
                return ApprovalDecision::Reject;
            }
            let slice = remaining.min(Duration::from_millis(200));
            match decisions.recv_timeout(slice) {
                // Decisions for an earlier request are stale — keep waiting.
                Ok(message) if message.approval_id != approval_id => continue,
                Ok(message) => return message.decision,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => return ApprovalDecision::Reject,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use tempfile::{tempdir, TempDir};

    use crate::context::LlmRequest;
    use crate::llm::{AgentResponse, LlmError, LlmToolCall, LlmTurn, ModelUsage};
    use crate::state::step::Action;

    /// Replays a fixed script of tool calls, then finishes.
    struct ScriptedLlm {
        turns: Mutex<Vec<Action>>,
    }

    impl ScriptedLlm {
        fn boxed(actions: Vec<Action>) -> Box<dyn LlmClient> {
            Box::new(Self {
                turns: Mutex::new(actions),
            })
        }
    }

    #[async_trait]
    impl LlmClient for ScriptedLlm {
        async fn respond(&self, _request: &LlmRequest) -> std::result::Result<LlmTurn, LlmError> {
            let action = {
                let mut turns = self.turns.lock().unwrap();
                if turns.is_empty() {
                    Action::Finish {
                        summary: "done".to_string(),
                    }
                } else {
                    turns.remove(0)
                }
            };
            Ok(LlmTurn {
                response: AgentResponse::ToolCalls(vec![LlmToolCall {
                    tool_call_id: format!("call_{}", uuid::Uuid::new_v4()),
                    action,
                }]),
                usage: ModelUsage::default(),
            })
        }

        fn model_id(&self) -> &str {
            "scripted"
        }
    }

    fn workspace() -> TempDir {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("notes.txt"), "hello\n").unwrap();
        dir
    }

    fn config(dir: &TempDir, mode: AgentMode, policy: EditPolicy) -> AgentConfig {
        AgentConfig::builder()
            .workspace(dir.path())
            .mode(mode)
            .edit_policy(policy)
            .provider("deepseek")
            .api_key("test-key")
            .model("scripted")
            .max_steps(6)
            .approval_timeout(Duration::from_secs(10))
            .build()
            .unwrap()
    }

    fn write_action() -> Action {
        Action::WriteFile {
            path: PathBuf::from("created.txt"),
            content: "written by the agent\n".to_string(),
        }
    }

    /// Drain the stream until the turn ends, answering approvals with `decision`.
    async fn run_turn(
        session: &AgentSession,
        events: &mut EventStream,
        decision: Option<ApprovalDecision>,
    ) -> (RunResult, Vec<AgentEvent>) {
        let mut seen = Vec::new();
        loop {
            let event = tokio::time::timeout(Duration::from_secs(30), events.recv())
                .await
                .expect("turn timed out")
                .expect("event stream closed before the turn finished");
            seen.push(event.clone());
            match event {
                AgentEvent::ApprovalRequired { request } => match decision {
                    Some(ApprovalDecision::ApproveForTask) => {
                        session.approve_for_task(request.id).await.unwrap()
                    }
                    Some(ApprovalDecision::ApproveOnce) => {
                        session.approve(request.id).await.unwrap()
                    }
                    _ => session.reject(request.id).await.unwrap(),
                },
                AgentEvent::Finished { result } => return (result, seen),
                _ => {}
            }
        }
    }

    #[tokio::test]
    async fn agent_mode_edits_after_approval() {
        let dir = workspace();
        let session = AgentSession::open_with_client(
            config(&dir, AgentMode::Agent, EditPolicy::Confirm),
            ScriptedLlm::boxed(vec![write_action()]),
        )
        .unwrap();
        let mut events = session.subscribe().unwrap();

        session.send("create created.txt").await.unwrap();
        let (result, seen) =
            run_turn(&session, &mut events, Some(ApprovalDecision::ApproveForTask)).await;

        assert!(result.success, "run should complete: {result:?}");
        assert!(dir.path().join("created.txt").exists());
        assert!(seen
            .iter()
            .any(|e| matches!(e, AgentEvent::ApprovalRequired { .. })));
        assert!(seen
            .iter()
            .any(|e| matches!(e, AgentEvent::FileChanged { path } if path.ends_with("created.txt"))));
        assert!(matches!(seen.first(), Some(AgentEvent::Started { .. })));
        session.shutdown().unwrap();
    }

    #[tokio::test]
    async fn approve_for_task_does_not_carry_to_the_next_send() {
        let dir = workspace();
        let session = AgentSession::open_with_client(
            config(&dir, AgentMode::Agent, EditPolicy::Confirm),
            ScriptedLlm::boxed(vec![
                write_action(),
                Action::Finish {
                    summary: "created created.txt".to_string(),
                },
                Action::WriteFile {
                    path: PathBuf::from("second.txt"),
                    content: "second\n".to_string(),
                },
                Action::Finish {
                    summary: "created second.txt".to_string(),
                },
            ]),
        )
        .unwrap();
        let mut events = session.subscribe().unwrap();

        session.send("create created.txt").await.unwrap();
        let (_, first) =
            run_turn(&session, &mut events, Some(ApprovalDecision::ApproveForTask)).await;
        assert!(first
            .iter()
            .any(|e| matches!(e, AgentEvent::ApprovalRequired { .. })));
        assert!(dir.path().join("created.txt").exists());

        // New send must ask again even after ApproveForTask on the previous turn.
        session.send("create second.txt").await.unwrap();
        let (_, second) =
            run_turn(&session, &mut events, Some(ApprovalDecision::ApproveForTask)).await;
        assert!(
            second
                .iter()
                .any(|e| matches!(e, AgentEvent::ApprovalRequired { .. })),
            "expected a fresh approval on the next send, got: {second:?}"
        );
        assert!(dir.path().join("second.txt").exists());
        session.shutdown().unwrap();
    }

    #[tokio::test]
    async fn rejected_edit_leaves_the_workspace_untouched() {
        let dir = workspace();
        let session = AgentSession::open_with_client(
            config(&dir, AgentMode::Agent, EditPolicy::Confirm),
            ScriptedLlm::boxed(vec![write_action()]),
        )
        .unwrap();
        let mut events = session.subscribe().unwrap();

        session.send("create created.txt").await.unwrap();
        let (_, seen) = run_turn(&session, &mut events, Some(ApprovalDecision::Reject)).await;

        assert!(!dir.path().join("created.txt").exists());
        assert!(seen
            .iter()
            .any(|e| matches!(e, AgentEvent::ApprovalRequired { .. })));
        assert!(!seen
            .iter()
            .any(|e| matches!(e, AgentEvent::FileChanged { .. })));
        session.shutdown().unwrap();
    }

    #[tokio::test]
    async fn ask_mode_never_asks_and_never_writes() {
        let dir = workspace();
        let session = AgentSession::open_with_client(
            // Ask mode downgrades Auto to read-only.
            config(&dir, AgentMode::Ask, EditPolicy::Auto),
            ScriptedLlm::boxed(vec![
                Action::ReadFile {
                    path: PathBuf::from("notes.txt"),
                    offset: 1,
                    limit: 50,
                },
                write_action(),
            ]),
        )
        .unwrap();
        let mut events = session.subscribe().unwrap();

        session.send("what does notes.txt contain?").await.unwrap();
        let (result, seen) = run_turn(&session, &mut events, None).await;

        assert_eq!(session.edit_policy(), EditPolicy::ReadOnly);
        // Nothing to verify in Ask mode, so an answered question is Completed.
        assert_eq!(result.status, RunStatus::Completed);
        assert!(!dir.path().join("created.txt").exists());
        assert!(!seen
            .iter()
            .any(|e| matches!(e, AgentEvent::ApprovalRequired { .. })));
        assert!(!seen
            .iter()
            .any(|e| matches!(e, AgentEvent::FileChanged { .. })));
        session.shutdown().unwrap();
    }

    #[tokio::test]
    async fn task_memory_persists_across_sends() {
        let dir = workspace();
        let session = AgentSession::open_with_client(
            config(&dir, AgentMode::Agent, EditPolicy::Auto),
            ScriptedLlm::boxed(vec![Action::UpdatePlan {
                objective: "explain notes.txt".to_string(),
                steps: "[x] read notes.txt\n[ ] answer".to_string(),
            }]),
        )
        .unwrap();
        let mut events = session.subscribe().unwrap();

        session.send("plan the work").await.unwrap();
        let (first, seen) = run_turn(&session, &mut events, None).await;
        assert!(first.success);
        let plan = seen.iter().find_map(|e| match e {
            AgentEvent::PlanUpdated { objective, steps, .. } => Some((objective.clone(), steps.clone())),
            _ => None,
        });
        let (objective, steps) = plan.expect("plan event");
        assert_eq!(objective, "explain notes.txt");
        assert_eq!(steps.len(), 2);
        assert!(steps[0].done);

        // Second turn on the same session: prior messages are still in memory.
        session.send("now answer").await.unwrap();
        let (second, _) = run_turn(&session, &mut events, None).await;
        assert!(second.success);
        assert_ne!(first.run_id, second.run_id);
        session.shutdown().unwrap();
    }

    #[tokio::test]
    async fn event_stream_has_a_single_consumer() {
        let dir = workspace();
        let session = AgentSession::open_with_client(
            config(&dir, AgentMode::Ask, EditPolicy::ReadOnly),
            ScriptedLlm::boxed(vec![]),
        )
        .unwrap();

        assert!(session.subscribe().is_ok());
        let err = session.subscribe().unwrap_err().to_string();
        assert!(err.contains("already taken"), "{err}");
        session.shutdown().unwrap();
    }

    #[tokio::test]
    async fn empty_message_is_rejected() {
        let dir = workspace();
        let session = AgentSession::open_with_client(
            config(&dir, AgentMode::Ask, EditPolicy::ReadOnly),
            ScriptedLlm::boxed(vec![]),
        )
        .unwrap();

        assert!(session.send("   ").await.is_err());
        session.shutdown().unwrap();
    }
}