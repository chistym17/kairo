use std::path::PathBuf;

use serde_json::Value;

use crate::session::Message;
use crate::state::step::Step;

/// Workspace information included in the agent context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceContext {
    pub root: PathBuf,
    pub instructions: Option<String>,
}

/// Complete context assembled for one LLM decision.
#[derive(Debug, Clone)]
pub struct AgentContext {
    pub system_prompt: String,
    pub session_messages: Vec<Message>,
    pub goal: String,
    pub run_summary: Option<String>,
    pub step_history: Vec<Step>,
    pub tools: Value,
    pub workspace: WorkspaceContext,
}

/// Ready-to-send payload for the LLM client.
#[derive(Debug, Clone)]
pub struct LlmRequest {
    pub messages: Value,
    pub tools: Value,
}
