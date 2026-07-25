use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProjectKind {
    Frontend,
    Backend,
    Library,
    Cli,
    Worker,
    Infrastructure,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CommandHint {
    pub label: String,
    pub program: String,
    pub args: Vec<String>,
    pub working_directory: PathBuf,
    pub confidence: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EntryPoint {
    pub path: PathBuf,
    pub score: u32,
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectUnit {
    pub path: PathBuf,
    pub kind: Option<ProjectKind>,
    pub framework: Option<String>,
    pub manifests: Vec<PathBuf>,
    pub commands: Vec<CommandHint>,
    pub entry_points: Vec<EntryPoint>,
    pub confidence: f32,
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectSnapshot {
    pub root: PathBuf,
    pub units: Vec<ProjectUnit>,
    pub truncated: bool,
}

impl ProjectKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProjectKind::Frontend => "frontend",
            ProjectKind::Backend => "backend",
            ProjectKind::Library => "library",
            ProjectKind::Cli => "cli",
            ProjectKind::Worker => "worker",
            ProjectKind::Infrastructure => "infrastructure",
            ProjectKind::Unknown => "unknown",
        }
    }
}
