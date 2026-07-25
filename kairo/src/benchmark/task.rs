use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Task {
    pub id: String,
    pub tier: u32,
    pub workspace: PathBuf,
    #[serde(default)]
    pub git_ref: Option<String>,
    pub goal: String,
    pub verify_cmd: String,
    #[serde(default)]
    pub files_readonly: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct TaskFile {
    tasks: Vec<Task>,
}

pub fn load_tasks(path: &Path) -> Result<Vec<Task>> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read task file: {}", path.display()))?;
    let file: TaskFile = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse task file: {}", path.display()))?;

    for task in &file.tasks {
        anyhow::ensure!(!task.id.is_empty(), "task id must not be empty");
        anyhow::ensure!(
            !task.goal.is_empty(),
            "task {} must have a non-empty goal",
            task.id
        );
    }

    Ok(file.tasks)
}

impl Task {
    pub fn resolved_git_ref(&self) -> String {
        self.git_ref
            .clone()
            .unwrap_or_else(|| format!("task/{}", self.id))
    }
}
