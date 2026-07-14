use std::collections::BTreeSet;
use std::path::PathBuf;

use anyhow::Result;

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
    pub success: bool,
    pub summary: Option<String>,
    pub changed_files: Vec<String>,
    pub log_path: PathBuf,
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

    pub async fn run(
        &self,
        goal: impl Into<String>,
        workspace_root: impl Into<PathBuf>,
        observer: Option<&dyn AgentObserver>,
        mut session: Option<&mut Session>,
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
        )
        .await;

        if let Some(session) = session {
            session.add_user_message(&goal);
            if let Some(summary) = finish_summary(&state) {
                session.add_assistant_message(summary);
            }
        }

        let changed_files = collect_changed_files(&state);
        let log_path = PathBuf::from("logs").join(format!("run_{}.jsonl", state.run_id));
        let summary = finish_summary(&state);

        let result = RunResult {
            run_id: state.run_id,
            stop_reason: stop_reason.clone(),
            total_steps: state.step_count,
            success: matches!(
                stop_reason,
                StopReason::TaskComplete | StopReason::CompletedUnverified
            ),
            summary,
            changed_files,
            log_path,
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
    use crate::llm::{LlmClient, LlmError, LlmToolCall};

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
        async fn next_actions(
            &self,
            _request: &LlmRequest,
        ) -> std::result::Result<Vec<LlmToolCall>, LlmError> {
            let mut batches = self.batches.lock().unwrap();
            let actions = if batches.is_empty() {
                vec![Action::Finish {
                    summary: "done".to_string(),
                }]
            } else {
                batches.remove(0)
            };
            Ok(actions
                .into_iter()
                .map(|action| LlmToolCall {
                    tool_call_id: format!("call_fake_{}", uuid::Uuid::new_v4()),
                    action,
                })
                .collect())
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

        let result = runtime.run("goal", &workspace, None, None).await.unwrap();
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

        let result = runtime.run("goal", &workspace, None, None).await.unwrap();
        assert!(result.success);
        assert_eq!(result.total_steps, 4); // 3 parallel reads + finish

        let log = std::fs::read_to_string(&result.log_path).unwrap();
        assert!(
            log.contains("parallel_tool_group"),
            "expected parallel_tool_group event in log"
        );
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

        let result = runtime.run("goal", &workspace, None, None).await.unwrap();
        assert!(result.success);
        let log = std::fs::read_to_string(&result.log_path).unwrap();
        assert!(log.contains("cache_miss"), "first read should miss");
        assert!(log.contains("cache_hit"), "second identical read should hit");
        assert!(log.contains("[cached — same unchanged range as step"));
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

        let result = runtime.run("goal", &workspace, None, None).await.unwrap();
        assert!(result.success);
        let log = std::fs::read_to_string(&result.log_path).unwrap();
        assert!(log.contains("cache_invalidate"));
        let miss_count = log.matches("cache_miss").count();
        let hit_count = log.matches("cache_hit").count();
        assert!(
            miss_count >= 2,
            "expected miss before and after write, got {miss_count}"
        );
        assert_eq!(hit_count, 0, "post-write read must not hit stale cache");
        assert_eq!(
            std::fs::read_to_string(workspace.join("a.txt")).unwrap(),
            "v2\n"
        );
    }
}

