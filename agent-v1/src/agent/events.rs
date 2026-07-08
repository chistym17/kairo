use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    RunStarted {
        run_id: String,
        goal: String,
        workspace_root: String,
        model_id: String,
        max_steps: u32,
    },
    RunFinished {
        run_id: String,
        stop_reason: String,
        total_steps: u32,
        success: bool,
    },
    StepStarted {
        run_id: String,
        step_index: u32,
    },
    StepCompleted {
        run_id: String,
        step_index: u32,
        stop_reason: Option<String>,
    },
    ToolStarted {
        run_id: String,
        step_index: u32,
        tool: String,
    },
    ToolCompleted {
        run_id: String,
        step_index: u32,
        tool: String,
        success: bool,
    },
    FileModified {
        run_id: String,
        step_index: u32,
        path: String,
    },
    CommandCompleted {
        run_id: String,
        step_index: u32,
        cmd: String,
        success: bool,
    },
    Error {
        run_id: String,
        step_index: Option<u32>,
        message: String,
    },
}

