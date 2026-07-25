use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};
use chrono::Utc;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct RunIndexEntry {
    pub run_id: String,
    pub task_id: String,
    pub trial: u32,
    pub timestamp: String,
}

pub fn append_entry(output_dir: &Path, entry: &RunIndexEntry) -> Result<()> {
    fs::create_dir_all(output_dir)?;

    let index_path = output_dir.join("run_index.jsonl");
    let line = serde_json::to_string(entry).context("failed to serialize run index entry")?;

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&index_path)
        .with_context(|| format!("failed to open {}", index_path.display()))?;

    writeln!(file, "{line}")?;
    Ok(())
}

pub fn record_run(output_dir: &Path, run_id: &str, task_id: &str, trial: u32) -> Result<()> {
    append_entry(
        output_dir,
        &RunIndexEntry {
            run_id: run_id.to_string(),
            task_id: task_id.to_string(),
            trial,
            timestamp: Utc::now().to_rfc3339(),
        },
    )
}
