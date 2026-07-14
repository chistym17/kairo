use std::path::PathBuf;
use uuid::Uuid;

use crate::state::plan::AgentPlan;
use crate::state::step::Step;
use crate::tools::project_snapshot::ProjectSnapshot;

#[derive(Debug)]
pub struct AgentState {
    pub run_id: String,
    pub model_id: String,

    pub goal: String,
    pub workspace_root: PathBuf,
    pub history: Vec<Step>,
    pub step_count: u32,
    /// Number of LLM responses (a turn may include multiple tool calls).
    pub model_turns: u32,
    pub max_steps: u32,

    /// Optional working plan for multi-step tasks (Phase 7.3).
    pub plan: Option<AgentPlan>,

    /// Orientation snapshot (computed once per run; refreshable via tool).
    pub project_snapshot: Option<ProjectSnapshot>,
}

impl AgentState {
    pub fn new(
        goal: impl Into<String>,
        workspace_root: impl Into<PathBuf>,
        model_id: impl Into<String>,
        max_steps: u32,
    ) -> Self {
        Self {
            run_id: Uuid::new_v4().to_string(),
            model_id: model_id.into(),
            goal: goal.into(),
            workspace_root: workspace_root.into(),
            history: Vec::new(),
            step_count: 0,
            model_turns: 0,
            max_steps,
            plan: None,
            project_snapshot: None,
        }
    }

    pub fn is_exhausted(&self) -> bool {
        self.model_turns >= self.max_steps
    }

    pub fn record_step(&mut self, step: Step) {
        self.history.push(step);
        self.step_count += 1;
    }
}
