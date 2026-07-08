use std::collections::BTreeSet;
use std::path::PathBuf;

use anyhow::Result;

use crate::llm::LlmClient;
use crate::loop_engine;
use crate::agent::{AgentEvent, AgentObserver};
use crate::state::agent_state::AgentState;
use crate::state::step::Action;
use crate::state::stop_reason::StopReason;

#[derive(Debug, Clone)]
pub struct RunResult {
    pub run_id: String,
    pub stop_reason: StopReason,
    pub total_steps: u32,
    pub success: bool,
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
    pub system_prompt: String,
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
    ) -> Self {
        Self {
            llm,
            model_id: model_id.into(),
            max_steps,
            step_delay_ms,
            tool_use_max_retries,
            tool_use_retry_delay_ms,
            system_prompt: system_prompt.into(),
        }
    }

    pub async fn run(
        &self,
        goal: impl Into<String>,
        workspace_root: impl Into<PathBuf>,
        observer: Option<&dyn AgentObserver>,
    ) -> Result<RunResult> {
        let goal = goal.into();
        let workspace_root = workspace_root.into();

        anyhow::ensure!(
            workspace_root.exists(),
            "workspace_root does not exist: {}",
            workspace_root.display()
        );

        let mut state = AgentState::new(
            goal,
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
            &self.system_prompt,
            self.step_delay_ms,
            self.tool_use_max_retries,
            self.tool_use_retry_delay_ms,
            observer,
        )
        .await;

        let changed_files = collect_changed_files(&state);
        let log_path = PathBuf::from("logs").join(format!("run_{}.jsonl", state.run_id));

        let result = RunResult {
            run_id: state.run_id,
            stop_reason: stop_reason.clone(),
            total_steps: state.step_count,
            success: stop_reason == StopReason::TaskComplete,
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

    use crate::llm::{LlmError};
    use crate::state::step::Step;

    struct FakeLlm {
        actions: std::sync::Mutex<Vec<Action>>,
    }

    impl FakeLlm {
        fn new(actions: Vec<Action>) -> Self {
            Self {
                actions: std::sync::Mutex::new(actions),
            }
        }
    }

    #[async_trait]
    impl LlmClient for FakeLlm {
        async fn next_action(
            &self,
            _system_prompt: &str,
            _goal: &str,
            _history: &[Step],
        ) -> std::result::Result<Action, LlmError> {
            let mut actions = self.actions.lock().unwrap();
            if actions.is_empty() {
                return Ok(Action::Finish {
                    summary: "done".to_string(),
                });
            }
            Ok(actions.remove(0))
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
        );

        let result = runtime.run("goal", &workspace, None).await.unwrap();
        assert!(result.success);
        assert!(result.changed_files.iter().any(|p| p.ends_with("hello.txt")));
    }
}

