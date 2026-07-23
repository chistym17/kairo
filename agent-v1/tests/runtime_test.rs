use std::path::PathBuf;

use agent_v1::agent::AgentRuntime;
use agent_v1::context::LlmRequest;
use agent_v1::llm::{AgentResponse, LlmClient, LlmError, LlmToolCall, LlmTurn, ModelUsage};
use agent_v1::state::step::Action;
use async_trait::async_trait;

struct FakeLlm {
    actions: std::sync::Mutex<Vec<Action>>,
    rate_limit_once: std::sync::atomic::AtomicBool,
}

impl FakeLlm {
    fn new(actions: Vec<Action>) -> Self {
        Self {
            actions: std::sync::Mutex::new(actions),
            rate_limit_once: std::sync::atomic::AtomicBool::new(false),
        }
    }

    fn with_rate_limit_once(self) -> Self {
        self.rate_limit_once
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self
    }
}

#[async_trait]
impl LlmClient for FakeLlm {
    async fn respond(&self, _request: &LlmRequest) -> Result<LlmTurn, LlmError> {
        if self
            .rate_limit_once
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Err(LlmError::RateLimited);
        }

        let mut actions = self.actions.lock().unwrap();
        let action = if actions.is_empty() {
            Action::Finish {
                summary: "done".to_string(),
            }
        } else {
            actions.remove(0)
        };
        Ok(LlmTurn {
            response: AgentResponse::ToolCalls(vec![LlmToolCall {
                tool_call_id: format!("call_fake_{}", uuid::Uuid::new_v4()),
                action,
            }]),
            usage: ModelUsage::default(),
        })
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
        10,
        0,
        0,
        0,
        "system",
        16000,
        2048,
        20,
    );

    let result = runtime.run("goal", workspace, None, None, None).await.unwrap();

    assert!(result.success);
    assert!(result.total_steps >= 1);
    assert!(result.changed_files.iter().any(|p| p.ends_with("hello.txt")));
    assert!(result.log_path.to_string_lossy().contains("logs/run_"));
}

#[tokio::test]
async fn rate_limit_does_not_consume_step_budget() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().to_path_buf();

    let llm = FakeLlm::new(vec![
        Action::RunCommand {
            cmd: "ls".to_string(),
            cwd: PathBuf::from("."),
            timeout_secs: 30,
        },
        Action::Finish {
            summary: "ok".to_string(),
        },
    ])
    .with_rate_limit_once();

    let runtime = AgentRuntime::new(
        Box::new(llm),
        "fake",
        2,
        0,
        0,
        0,
        "system",
        16000,
        2048,
        20,
    );

    let result = runtime.run("goal", workspace, None, None, None).await.unwrap();
    assert!(result.success);
    assert_eq!(result.total_steps, 2);
}

