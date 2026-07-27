use std::collections::BTreeSet;
use std::path::PathBuf;

use anyhow::Result;

use crate::authorization::ApprovalGate;
use crate::llm::LlmClient;
use crate::loop_engine;
use crate::agent::{AgentEvent, AgentObserver};
use crate::context::{ContextBuilder, ContextManager};
use crate::session::Session;
use crate::state::agent_state::AgentState;
use crate::state::step::Action;
use crate::state::stop_reason::StopReason;

#[derive(Debug, Clone)]
pub struct RunResult {
    pub run_id: String,
    pub stop_reason: StopReason,
    pub total_steps: u32,
    pub total_model_turns: u32,
    pub success: bool,
    pub summary: Option<String>,
    pub changed_files: Vec<String>,
    pub log_path: PathBuf,
    pub efficiency: crate::state::RunEfficiency,
    pub usage: crate::llm::UsageTotals,
}

pub struct AgentRuntime {
    llm: Box<dyn LlmClient>,
    pub model_id: String,
    pub max_steps: u32,
    pub step_delay_ms: u64,
    pub tool_use_max_retries: u32,
    pub tool_use_retry_delay_ms: u64,
    /// When true (benchmarks), finish requires a prior successful run_command/run_shell.
    pub require_verified_finish: bool,
    /// When true, enable semantic governance (modes / tool gating). Default false.
    pub semantic_policy: bool,
    context_builder: ContextBuilder,
    context_manager: ContextManager,
}

impl AgentRuntime {
    pub fn new(
        llm: Box<dyn LlmClient>,
        model_id: impl Into<String>,
        max_steps: u32,
        step_delay_ms: u64,
        tool_use_max_retries: u32,
        tool_use_retry_delay_ms: u64,
        system_prompt: impl Into<String>,
        max_context_tokens: usize,
        context_reserve_tokens: usize,
        keep_recent_session_messages: usize,
    ) -> Self {
        Self {
            llm,
            model_id: model_id.into(),
            max_steps,
            step_delay_ms,
            tool_use_max_retries,
            tool_use_retry_delay_ms,
            // Developer/default: allow finish without a green verify step.
            require_verified_finish: false,
            semantic_policy: false,
            context_builder: ContextBuilder::new().with_system_prompt(system_prompt),
            context_manager: ContextManager::new(
                max_context_tokens,
                context_reserve_tokens,
                keep_recent_session_messages,
            ),
        }
    }

    pub fn with_verified_finish(mut self, require: bool) -> Self {
        self.require_verified_finish = require;
        self
    }

    pub fn with_semantic_policy(mut self, enabled: bool) -> Self {
        self.semantic_policy = enabled;
        self
    }

    pub async fn run(
        &self,
        goal: impl Into<String>,
        workspace_root: impl Into<PathBuf>,
        observer: Option<&dyn AgentObserver>,
        session: Option<&mut Session>,
        approval_gate: Option<&dyn ApprovalGate>,
    ) -> Result<RunResult> {
        self.run_with_cancel(goal, workspace_root, observer, session, approval_gate, None)
            .await
    }

    pub async fn run_with_cancel(
        &self,
        goal: impl Into<String>,
        workspace_root: impl Into<PathBuf>,
        observer: Option<&dyn AgentObserver>,
        mut session: Option<&mut Session>,
        approval_gate: Option<&dyn ApprovalGate>,
        cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    ) -> Result<RunResult> {
        let goal = goal.into();
        let workspace_root = workspace_root.into();

        anyhow::ensure!(
            workspace_root.exists(),
            "workspace_root does not exist: {}",
            workspace_root.display()
        );

        let mut state = AgentState::new(
            goal.clone(),
            workspace_root,
            self.model_id.clone(),
            self.max_steps,
        );
        state.governance.semantic_policy = self.semantic_policy;
        if let Some(cancel) = cancel {
            state = state.with_cancel(cancel);
        }

        if let Some(obs) = observer {
            obs.on_event(AgentEvent::RunStarted {
                run_id: state.run_id.clone(),
                goal: state.goal.clone(),
                workspace_root: state.workspace_root.to_string_lossy().to_string(),
                model_id: state.model_id.clone(),
                max_steps: state.max_steps,
            });
        }

        let stop_reason = loop_engine::run(
            &mut state,
            self.llm.as_ref(),
            &self.context_builder,
            &self.context_manager,
            self.step_delay_ms,
            self.tool_use_max_retries,
            self.tool_use_retry_delay_ms,
            self.require_verified_finish,
            observer,
            session.as_deref_mut(),
            approval_gate,
        )
        .await;

        if let Some(session) = session {
            // Persist final authorization grants and durable task state.
            session.authorization = state.authorization.clone();
            session.task = state.task.clone();
            session.declared_intent = state.turn_intent;
            session.add_user_message(&goal);
            if let Some(summary) = finish_summary(&state) {
                session.add_assistant_message(summary);
            }
        }

        let changed_files = collect_changed_files(&state);
        let log_path = PathBuf::from("logs").join(format!("run_{}.jsonl", state.run_id));
        let summary = finish_summary(&state);
        let efficiency = state.efficiency_snapshot();

        let result = RunResult {
            run_id: state.run_id,
            stop_reason: stop_reason.clone(),
            total_steps: efficiency.total_steps,
            total_model_turns: efficiency.total_model_turns,
            success: matches!(
                stop_reason,
                StopReason::TaskComplete | StopReason::CompletedUnverified
            ),
            summary,
            changed_files,
            log_path,
            efficiency,
            usage: state.model_usage.totals(),
        };

        if let Some(obs) = observer {
            obs.on_event(AgentEvent::RunFinished {
                run_id: result.run_id.clone(),
                stop_reason: format!("{:?}", result.stop_reason),
                total_steps: result.total_steps,
                success: result.success,
            });
        }

        Ok(result)
    }
}

fn finish_summary(state: &AgentState) -> Option<String> {
    state.history.iter().rev().find_map(|step| {
        if let Action::Finish { summary } = &step.action {
            Some(summary.clone())
        } else {
            None
        }
    })
}

fn collect_changed_files(state: &AgentState) -> Vec<String> {
    let mut set = BTreeSet::new();
    for step in &state.history {
        match &step.action {
            Action::ApplyPatch { path, .. } | Action::WriteFile { path, .. } => {
                set.insert(path.to_string_lossy().to_string());
            }
            _ => {}
        }
    }
    set.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use tempfile::tempdir;

    use crate::context::LlmRequest;
    use crate::llm::{AgentResponse, LlmClient, LlmError, LlmToolCall, LlmTurn};
    use crate::tools::shell_cache;

    struct FakeLlm {
        batches: std::sync::Mutex<Vec<Vec<Action>>>,
    }

    impl FakeLlm {
        fn new(actions: Vec<Action>) -> Self {
            Self {
                batches: std::sync::Mutex::new(
                    actions.into_iter().map(|a| vec![a]).collect(),
                ),
            }
        }

        fn batches(batches: Vec<Vec<Action>>) -> Self {
            Self {
                batches: std::sync::Mutex::new(batches),
            }
        }
    }

    #[async_trait]
    impl LlmClient for FakeLlm {
        async fn respond(
            &self,
            _request: &LlmRequest,
        ) -> std::result::Result<LlmTurn, LlmError> {
            let mut batches = self.batches.lock().unwrap();
            let actions = if batches.is_empty() {
                vec![Action::Finish {
                    summary: "done".to_string(),
                }]
            } else {
                batches.remove(0)
            };
            Ok(LlmTurn {
                response: AgentResponse::ToolCalls(
                    actions
                        .into_iter()
                        .map(|action| LlmToolCall {
                            tool_call_id: format!("call_fake_{}", uuid::Uuid::new_v4()),
                            action,
                        })
                        .collect(),
                ),
                usage: crate::llm::ModelUsage::default(),
            })
        }

        fn model_id(&self) -> &str {
            "fake"
        }
    }

    #[tokio::test]
    async fn runtime_returns_changed_files() {
        let dir = tempdir().unwrap();
        let workspace = dir.path().to_path_buf();

        let llm = FakeLlm::new(vec![
            Action::WriteFile {
                path: PathBuf::from("hello.txt"),
                content: "hi".to_string(),
            },
            Action::RunCommand {
                cmd: "ls".to_string(),
                cwd: PathBuf::from("."),
                timeout_secs: 30,
            },
            Action::Finish {
                summary: "ok".to_string(),
            },
        ]);

        let runtime = AgentRuntime::new(
            Box::new(llm),
            "fake",
            5,
            0,
            0,
            0,
            "system",
            16000,
            2048,
            20,
        );

        let result = runtime.run("goal", &workspace, None, None, None).await.unwrap();
        assert!(result.success);
        assert!(result.changed_files.iter().any(|p| p.ends_with("hello.txt")));
    }

    #[tokio::test]
    async fn parallel_readonly_batch_completes() {
        let dir = tempdir().unwrap();
        let workspace = dir.path().to_path_buf();
        std::fs::write(workspace.join("a.txt"), "aaa\n").unwrap();
        std::fs::write(workspace.join("b.txt"), "bbb\n").unwrap();

        let llm = FakeLlm::batches(vec![
            vec![
                Action::ReadFile {
                    path: PathBuf::from("a.txt"),
                    offset: 1,
                    limit: 50,
                },
                Action::ReadFile {
                    path: PathBuf::from("b.txt"),
                    offset: 1,
                    limit: 50,
                },
                Action::ListFiles {
                    path: PathBuf::from("."),
                },
            ],
            vec![Action::Finish {
                summary: "explored".to_string(),
            }],
        ]);

        let runtime = AgentRuntime::new(
            Box::new(llm),
            "fake",
            5,
            0,
            0,
            0,
            "system",
            16000,
            2048,
            20,
        );

        let result = runtime.run("goal", &workspace, None, None, None).await.unwrap();
        assert!(result.success);
        assert_eq!(result.total_steps, 4); // 3 parallel reads + finish
        assert_eq!(result.efficiency.parallel_tool_groups, 1);
        assert_eq!(result.total_model_turns, 2);

        let log = std::fs::read_to_string(&result.log_path).unwrap();
        assert!(
            log.contains("parallel_tool_group"),
            "expected parallel_tool_group event in log"
        );
        assert!(log.contains("\"parallel_tool_groups\":1") || log.contains("\"parallel_tool_groups\": 1"));
    }

    #[tokio::test]
    async fn read_cache_hits_on_identical_reread() {
        let dir = tempdir().unwrap();
        let workspace = dir.path().to_path_buf();
        std::fs::write(workspace.join("a.txt"), "hello cache\n").unwrap();

        let llm = FakeLlm::batches(vec![
            vec![Action::ReadFile {
                path: PathBuf::from("a.txt"),
                offset: 1,
                limit: 50,
            }],
            vec![Action::ReadFile {
                path: PathBuf::from("a.txt"),
                offset: 1,
                limit: 50,
            }],
            vec![Action::Finish {
                summary: "done".to_string(),
            }],
        ]);

        let runtime = AgentRuntime::new(
            Box::new(llm),
            "fake",
            5,
            0,
            0,
            0,
            "system",
            16000,
            2048,
            20,
        );

        let result = runtime.run("goal", &workspace, None, None, None).await.unwrap();
        assert!(result.success);
        let log = std::fs::read_to_string(&result.log_path).unwrap();
        assert!(log.contains("\"event\":\"cache_miss\"") || log.contains("\"event\": \"cache_miss\""));
        assert!(log.contains("\"event\":\"cache_hit\"") || log.contains("\"event\": \"cache_hit\""));
        assert!(
            log.contains("Duplicate read denied") || log.contains("duplicate read denied"),
            "expected denied duplicate read in log"
        );
        assert_eq!(result.efficiency.cache_hits, 1);
        assert!(result.efficiency.cache_misses >= 1);
    }

    #[tokio::test]
    async fn read_cache_invalidates_after_write() {
        let dir = tempdir().unwrap();
        let workspace = dir.path().to_path_buf();
        std::fs::write(workspace.join("a.txt"), "v1\n").unwrap();

        let llm = FakeLlm::batches(vec![
            vec![Action::ReadFile {
                path: PathBuf::from("a.txt"),
                offset: 1,
                limit: 50,
            }],
            vec![Action::WriteFile {
                path: PathBuf::from("a.txt"),
                content: "v2\n".to_string(),
            }],
            vec![Action::ReadFile {
                path: PathBuf::from("a.txt"),
                offset: 1,
                limit: 50,
            }],
            vec![Action::Finish {
                summary: "done".to_string(),
            }],
        ]);

        let runtime = AgentRuntime::new(
            Box::new(llm),
            "fake",
            10,
            0,
            0,
            0,
            "system",
            16000,
            2048,
            20,
        );

        let result = runtime.run("goal", &workspace, None, None, None).await.unwrap();
        assert!(result.success);
        let log = std::fs::read_to_string(&result.log_path).unwrap();
        assert!(log.contains("cache_invalidate"));
        let miss_count = log.matches("\"event\":\"cache_miss\"").count()
            + log.matches("\"event\": \"cache_miss\"").count();
        let hit_count = log.matches("\"event\":\"cache_hit\"").count()
            + log.matches("\"event\": \"cache_hit\"").count();
        assert!(
            miss_count >= 2,
            "expected miss before and after write, got {miss_count}"
        );
        assert_eq!(hit_count, 0, "post-write read must not hit stale cache");
        assert_eq!(
            std::fs::read_to_string(workspace.join("a.txt")).unwrap(),
            "v2\n"
        );
        assert!(result.efficiency.time_to_first_edit_ms.is_some());
        assert_eq!(result.efficiency.cache_hits, 0);
    }

    #[tokio::test]
    async fn identical_shell_is_cached_then_blocked() {
        let dir = tempdir().unwrap();
        let workspace = dir.path().to_path_buf();

        let llm = FakeLlm::batches(vec![
            vec![Action::RunShell {
                command: "echo hello-shell-dedup".to_string(),
                cwd: PathBuf::from("."),
                timeout_secs: 30,
            }],
            vec![Action::RunShell {
                command: "echo hello-shell-dedup".to_string(),
                cwd: PathBuf::from("."),
                timeout_secs: 30,
            }],
            vec![Action::RunShell {
                command: "echo hello-shell-dedup".to_string(),
                cwd: PathBuf::from("."),
                timeout_secs: 30,
            }],
            vec![Action::Finish {
                summary: "stop after blocked repeat".to_string(),
            }],
        ]);

        let runtime = AgentRuntime::new(
            Box::new(llm),
            "fake",
            10,
            0,
            0,
            0,
            "system",
            16000,
            2048,
            20,
        );

        let result = runtime.run("goal", &workspace, None, None, None).await.unwrap();
        assert!(result.success);
        let log = std::fs::read_to_string(&result.log_path).unwrap();
        assert!(
            log.contains("shell_cache_hit"),
            "second identical shell should hit cache"
        );
        assert!(
            log.contains("shell_repeat_blocked"),
            "third identical shell should be blocked"
        );
        assert!(log.contains("[shell-cache"));
        assert!(log.contains("Blocked: identical"));
    }

    #[tokio::test]
    async fn shell_cache_clears_after_edit() {
        let dir = tempdir().unwrap();
        let workspace = dir.path().to_path_buf();
        std::fs::write(workspace.join("a.txt"), "v1\n").unwrap();

        let llm = FakeLlm::batches(vec![
            vec![Action::RunShell {
                command: "echo after-edit-ok".to_string(),
                cwd: PathBuf::from("."),
                timeout_secs: 30,
            }],
            vec![Action::WriteFile {
                path: PathBuf::from("a.txt"),
                content: "v2\n".to_string(),
            }],
            vec![Action::RunShell {
                command: "echo after-edit-ok".to_string(),
                cwd: PathBuf::from("."),
                timeout_secs: 30,
            }],
            vec![Action::Finish {
                summary: "ok".to_string(),
            }],
        ]);

        let runtime = AgentRuntime::new(
            Box::new(llm),
            "fake",
            10,
            0,
            0,
            0,
            "system",
            16000,
            2048,
            20,
        );

        let result = runtime.run("goal", &workspace, None, None, None).await.unwrap();
        assert!(result.success);
        let log = std::fs::read_to_string(&result.log_path).unwrap();
        assert!(
            !log.contains("shell_cache_hit") && !log.contains("shell_repeat_blocked"),
            "re-run after edit must execute live, got hits/blocks in log"
        );
    }

    #[tokio::test]
    async fn explore_shell_budget_blocks_git_thrash_after_edit() {
        let dir = tempdir().unwrap();
        let workspace = dir.path().to_path_buf();
        std::fs::write(workspace.join("a.txt"), "v1\n").unwrap();

        let mut batches = vec![vec![Action::WriteFile {
            path: PathBuf::from("a.txt"),
            content: "v2\n".to_string(),
        }]];
        // One over the explore budget with distinct git pathspecs.
        for i in 0..(shell_cache::MAX_EXPLORE_SHELLS_SINCE_EDIT + 2) {
            batches.push(vec![Action::RunShell {
                command: format!("git log --oneline -1 -- path{i}.ts"),
                cwd: PathBuf::from("."),
                timeout_secs: 30,
            }]);
        }
        batches.push(vec![Action::Finish {
            summary: "done".to_string(),
        }]);

        let llm = FakeLlm::batches(batches);
        let runtime = AgentRuntime::new(
            Box::new(llm),
            "fake",
            40,
            0,
            0,
            0,
            "system",
            16000,
            2048,
            20,
        );

        let result = runtime.run("goal", &workspace, None, None, None).await.unwrap();
        assert!(result.success);
        let log = std::fs::read_to_string(&result.log_path).unwrap();
        assert!(
            log.contains("shell_explore_budget_blocked"),
            "expected explore budget block after {} git shells",
            shell_cache::MAX_EXPLORE_SHELLS_SINCE_EDIT
        );
        assert!(log.contains("explore-shell budget"));
    }

    #[tokio::test]
    async fn governance_stops_grep_spiral_after_edit() {
        let dir = tempdir().unwrap();
        let workspace = dir.path().to_path_buf();
        std::fs::write(workspace.join("api.ts"), "export const x = 1;\n").unwrap();

        let mut batches = vec![vec![Action::WriteFile {
            path: PathBuf::from("api.ts"),
            content: "export const x = 2;\n".to_string(),
        }]];
        for _ in 0..12 {
            batches.push(vec![Action::RunCommand {
                cmd: "grep position api.ts".to_string(),
                cwd: PathBuf::from("."),
                timeout_secs: 30,
            }]);
        }
        batches.push(vec![Action::Finish {
            summary: "done".to_string(),
        }]);

        let llm = FakeLlm::batches(batches);
        let runtime = AgentRuntime::new(
            Box::new(llm),
            "fake",
            30,
            0,
            0,
            0,
            "system",
            16000,
            2048,
            20,
        )
        .with_semantic_policy(true);

        let result = runtime.run("goal", &workspace, None, None, None).await.unwrap();
        let log = std::fs::read_to_string(&result.log_path).unwrap();
        assert!(
            log.contains("governance_rejected")
                || log.contains("governance_terminate")
                || log.contains("phase_tool_denied")
                || log.contains("phase → act"),
            "expected governance or Act-phase tool restriction to stop grep spiral"
        );
        assert!(
            !log.contains("MaxStepsExceeded"),
            "should not burn full step budget"
        );
        assert!(
            result.total_model_turns < 25,
            "expected early stop, got {} turns",
            result.total_model_turns
        );
    }
}

