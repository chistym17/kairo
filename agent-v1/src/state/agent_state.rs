pub struct AgentState {
    pub run_id: String,
    pub model_id: String,
    pub goal: String,
    pub workspace_root: PathBuf,
    pub history: Vec<Step>,
    pub step_count: u32,
    pub max_steps: u32,
}
