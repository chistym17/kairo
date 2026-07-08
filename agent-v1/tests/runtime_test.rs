use std::path::PathBuf;

use agent_v1::agent::AgentRuntime;
use agent_v1::llm::{LlmClient, LlmError};
use agent_v1::state::step::{Action, Step};
use async_trait::async_trait;

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
    ) -> Result<Action, LlmError> {
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
async fn runtime_runs_and_reports_changed_files() {
    let dir = tempfile::tempdir().unwrap();
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
        10,
        0,
        0,
        0,
        "system",
    );

    let result = runtime.run("goal", workspace, None).await.unwrap();

    assert!(result.success);
    assert!(result.total_steps >= 1);
    assert!(result.changed_files.iter().any(|p| p.ends_with("hello.txt")));
    assert!(result.log_path.to_string_lossy().contains("logs/run_"));
}

