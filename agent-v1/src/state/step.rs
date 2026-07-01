pub enum Action {
    ReadFile { path: PathBuf },
    WriteFile { path: PathBuf, content: String },
    RunCommand { cmd: String },
    Finish { summary: String },
}

pub struct Step {
    pub step_index: u32,
    pub timestamp: String,
    pub action: Action,
    pub result: ToolResult,
}
