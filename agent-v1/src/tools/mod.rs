pub struct ToolResult {
    pub success: bool,
    pub output: String,
    pub diff: Option<String>,
    pub error: Option<String>,
    pub stop_reason: Option<StopReason>,
}

pub fn dispatch(action: &Action, workspace_root: &Path) -> ToolResult {
    match action {
        Action::ReadFile { path } => read_file::execute(path, workspace_root),
        Action::WriteFile { path, content } => write_file::execute(path, content, workspace_root),
        Action::RunCommand { cmd } => run_command::execute(cmd, workspace_root),
        Action::Finish { summary } => ToolResult {
            success: true,
            output: summary.clone(),
            diff: None,
            error: None,
            stop_reason: Some(StopReason::TaskComplete),
        },
    }
}
