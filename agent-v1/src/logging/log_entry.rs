use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::state::step::{Action, Step};
use crate::tools::ToolResult;

/// The on-disk representation of one agent step.
///
/// This is NOT the same as Step (which is the in-memory representation).
/// Step is what the loop engine works with.
/// LogEntry is what gets written to disk and what Phase 3's benchmark
/// parser reads to compute metrics.
///
/// Keeping them separate means:
///   - You can change Step's in-memory shape without breaking log parsing
///   - You can add log-only fields (run_id, model_id) without cluttering Step
///   - Phase 3 can read logs from multiple runs without knowing about AgentState
///
/// Schema is fixed from day 1. Changing field names later means
/// rewriting the Phase 3 parser. Don't change this casually.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    /// Ties all steps from one agent run together.
    /// Same value on every line for a single execution.
    pub run_id: String,

    /// Which model produced this action.
    /// Lets you query "all steps where model_id = llama-3.1-70b"
    /// across a benchmark run that used multiple models.
    pub model_id: String,

    /// 0-indexed position in the run.
    /// With run_id + step_index you can reconstruct any run in order.
    pub step_index: u32,

    /// Model turn that produced this tool call (one turn may yield many steps).
    #[serde(default)]
    pub model_turn: u32,

    /// ISO 8601 timestamp — when this step completed.
    pub timestamp: String,

    /// What the model decided to do.
    pub action: Action,

    /// What happened when we ran it.
    pub result: ToolResult,
}

impl LogEntry {
    /// Build a LogEntry from a completed Step plus run metadata.
    pub fn from_step(run_id: &str, model_id: &str, step: &Step) -> Self {
        Self {
            run_id: run_id.to_string(),
            model_id: model_id.to_string(),
            step_index: step.step_index,
            model_turn: step.model_turn,
            timestamp: step.timestamp.clone(),
            action: step.action.clone(),
            result: step.result.clone(),
        }
    }
}

/// Write a single LogEntry as one JSON line to the run's log file.
///
/// File is opened in append mode and closed after every write.
/// This is intentionally not buffered — if the agent crashes or
/// hits MAX_STEPS mid-run, every step up to that point is on disk.
/// Losing the last step is acceptable. Losing all steps is not.
///
/// Log file path: logs/run_{run_id}.jsonl
/// One file per run. Phase 3 iterates the logs/ directory and
/// processes each file as one benchmark trial.
pub fn write_entry(entry: &LogEntry) {
    let log_dir = PathBuf::from("logs");

    // Create logs/ directory if it doesn't exist
    if let Err(e) = std::fs::create_dir_all(&log_dir) {
        eprintln!("[logging] failed to create logs directory: {}", e);
        return;
    }

    let log_path = log_dir.join(format!("run_{}.jsonl", entry.run_id));

    // Serialize to JSON — single line, no pretty printing
    let line = match serde_json::to_string(entry) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[logging] failed to serialize log entry: {}", e);
            return;
        }
    };

    // Open in append mode — never truncate an existing log file
    let mut file = match OpenOptions::new().create(true).append(true).open(&log_path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("[logging] failed to open {}: {}", log_path.display(), e);
            return;
        }
    };

    // Write the line + newline — JSONL format requires one JSON object per line
    if let Err(e) = writeln!(file, "{}", line) {
        eprintln!("[logging] failed to write entry: {}", e);
    }
}

/// Write an event (delay, rate-limit retry, etc.) to the run's log file.
pub fn write_event(run_id: &str, event_type: &str, detail: &str, duration_ms: u64) {
    let log_dir = PathBuf::from("logs");
    let log_path = log_dir.join(format!("run_{}.jsonl", run_id));

    let event = serde_json::json!({
        "record_type": "event",
        "run_id": run_id,
        "event": event_type,
        "detail": detail,
        "duration_ms": duration_ms,
        "timestamp": chrono::Utc::now().to_rfc3339(),
    });

    let line = match serde_json::to_string(&event) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[logging] failed to serialize event: {}", e);
            return;
        }
    };

    let mut file = match OpenOptions::new().create(true).append(true).open(&log_path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("[logging] failed to open log for event: {}", e);
            return;
        }
    };

    if let Err(e) = writeln!(file, "{}", line) {
        eprintln!("[logging] failed to write event: {}", e);
    }
}

/// Write a run summary as the final line of the log file.
///
/// Called after the loop exits with a StopReason.
/// Phase 3 looks for this line to determine run outcome without
/// having to re-parse every step.
pub fn write_run_summary(
    run_id: &str,
    model_id: &str,
    stop_reason: &crate::state::stop_reason::StopReason,
    total_steps: u32,
    total_model_turns: u32,
) {
    let log_dir = PathBuf::from("logs");
    let log_path = log_dir.join(format!("run_{}.jsonl", run_id));

    let summary = serde_json::json!({
        "record_type": "run_summary",
        "run_id": run_id,
        "model_id": model_id,
        "stop_reason": stop_reason,
        "total_steps": total_steps,
        "total_model_turns": total_model_turns,
        "timestamp": chrono::Utc::now().to_rfc3339(),
    });

    let line = match serde_json::to_string(&summary) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[logging] failed to serialize run summary: {}", e);
            return;
        }
    };

    let mut file = match OpenOptions::new().create(true).append(true).open(&log_path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("[logging] failed to open log for summary: {}", e);
            return;
        }
    };

    if let Err(e) = writeln!(file, "{}", line) {
        eprintln!("[logging] failed to write run summary: {}", e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::step::Action;
    use crate::state::stop_reason::StopReason;
    use crate::tools::ToolResult;
    use std::path::PathBuf;

    fn make_test_entry() -> LogEntry {
        LogEntry {
            run_id: "test-run-123".to_string(),
            model_id: "llama-3.1-70b".to_string(),
            step_index: 0,
            model_turn: 0,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            action: Action::ReadFile {
                path: PathBuf::from("calc.py"),
                offset: 1,
                limit: 250,
            },
            result: ToolResult {
                success: true,
                output: "def add(a, b):\n    return a - b\n".to_string(),
                diff: None,
                error: None,
                stop_reason: None,
            },
        }
    }

    #[test]
    fn test_entry_serializes_to_valid_json() {
        let entry = make_test_entry();
        let json = serde_json::to_string(&entry).unwrap();

        // Must be valid JSON
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        // Must have expected fields
        assert_eq!(parsed["run_id"], "test-run-123");
        assert_eq!(parsed["step_index"], 0);
        assert_eq!(parsed["model_id"], "llama-3.1-70b");
    }

    #[test]
    fn test_write_creates_jsonl_file() {
        // Override log dir for test — write to temp location
        // Note: write_entry hardcodes "logs/" so this test verifies
        // the file gets created; in CI you'd want a configurable path.
        let entry = make_test_entry();
        write_entry(&entry);

        let log_path = PathBuf::from("logs").join(format!("run_{}.jsonl", entry.run_id));

        assert!(log_path.exists());

        // Read back and verify it's valid JSONL
        let content = std::fs::read_to_string(&log_path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert!(!lines.is_empty());

        // Every line must be valid JSON
        for line in &lines {
            let parsed: serde_json::Value = serde_json::from_str(line).unwrap();
            assert_eq!(parsed["run_id"], "test-run-123");
        }
    }

    #[test]
    fn test_multiple_writes_append_not_overwrite() {
        let mut entry = make_test_entry();
        entry.run_id = "test-append-456".to_string();

        // Ensure a clean slate if previous test runs left a file behind.
        let log_path = PathBuf::from("logs").join(format!("run_{}.jsonl", entry.run_id));
        let _ = std::fs::remove_file(&log_path);

        // Write same entry twice
        write_entry(&entry);
        entry.step_index = 1;
        write_entry(&entry);

        let content = std::fs::read_to_string(&log_path).unwrap();
        let lines: Vec<&str> = content.lines().filter(|l| !l.is_empty()).collect();

        // Should have exactly 2 lines
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn test_run_summary_written() {
        write_run_summary(
            "test-summary-789",
            "llama-3.1-70b",
            &StopReason::TaskComplete,
            5,
            3,
        );

        let log_path = PathBuf::from("logs/run_test-summary-789.jsonl");
        let content = std::fs::read_to_string(&log_path).unwrap();
        let last_line = content.lines().last().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(last_line).unwrap();

        assert_eq!(parsed["record_type"], "run_summary");
        assert_eq!(parsed["total_steps"], 5);
        assert_eq!(parsed["total_model_turns"], 3);
    }
}
