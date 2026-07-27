use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::RwLock;

use serde::{Deserialize, Serialize};

use crate::state::step::{Action, Step};
use crate::tools::ToolResult;

/// Optional override for tests (`set_logs_dir_override`).
static LOGS_DIR_OVERRIDE: RwLock<Option<PathBuf>> = RwLock::new(None);

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

/// Override the log directory (tests). Pass `None` to clear.
pub fn set_logs_dir_override(path: Option<PathBuf>) {
    if let Ok(mut guard) = LOGS_DIR_OVERRIDE.write() {
        *guard = path;
    }
}

/// Directory for run JSONL logs.
///
/// Priority:
/// 1. [`set_logs_dir_override`] (tests)
/// 2. `KAIRO_LOG_DIR`
/// 3. `$XDG_DATA_HOME/kairo/logs` or `~/.local/share/kairo/logs`
pub fn logs_dir() -> PathBuf {
    if let Ok(guard) = LOGS_DIR_OVERRIDE.read() {
        if let Some(p) = guard.as_ref() {
            return p.clone();
        }
    }
    if let Ok(p) = std::env::var("KAIRO_LOG_DIR") {
        let p = p.trim();
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    // Unit/integration tests must not write into the developer's XDG data dir.
    #[cfg(test)]
    {
        return std::env::temp_dir().join("kairo-test-logs");
    }
    #[cfg(not(test))]
    {
        data_dir().join("logs")
    }
}

fn data_dir() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        let xdg = xdg.trim();
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("kairo");
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".local").join("share").join("kairo")
}

/// Path for one run's JSONL file.
pub fn run_log_path(run_id: &str) -> PathBuf {
    logs_dir().join(format!("run_{run_id}.jsonl"))
}

fn ensure_logs_dir() -> Option<PathBuf> {
    let log_dir = logs_dir();
    if let Err(e) = std::fs::create_dir_all(&log_dir) {
        log_io_error(format!("failed to create logs directory: {e}"));
        return None;
    }
    Some(log_dir)
}

fn log_io_error(message: impl AsRef<str>) {
    // Product CLI is quiet by default — don't spam stderr on successful runs.
    if !crate::logging::is_quiet() {
        eprintln!("[logging] {}", message.as_ref());
    }
}

/// Write a single LogEntry as one JSON line to the run's log file.
///
/// File is opened in append mode and closed after every write.
/// This is intentionally not buffered — if the agent crashes or
/// hits MAX_STEPS mid-run, every step up to that point is on disk.
/// Losing the last step is acceptable. Losing all steps is not.
pub fn write_entry(entry: &LogEntry) {
    let Some(_log_dir) = ensure_logs_dir() else {
        return;
    };
    let log_path = run_log_path(&entry.run_id);

    let line = match serde_json::to_string(entry) {
        Ok(s) => s,
        Err(e) => {
            log_io_error(format!("failed to serialize log entry: {e}"));
            return;
        }
    };

    let mut file = match OpenOptions::new().create(true).append(true).open(&log_path) {
        Ok(f) => f,
        Err(e) => {
            log_io_error(format!("failed to open {}: {e}", log_path.display()));
            return;
        }
    };

    if let Err(e) = writeln!(file, "{}", line) {
        log_io_error(format!("failed to write entry: {e}"));
    }
}

/// Write an event (delay, rate-limit retry, etc.) to the run's log file.
pub fn write_event(run_id: &str, event_type: &str, detail: &str, duration_ms: u64) {
    let Some(_log_dir) = ensure_logs_dir() else {
        return;
    };
    let log_path = run_log_path(run_id);

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
            log_io_error(format!("failed to serialize event: {e}"));
            return;
        }
    };

    let mut file = match OpenOptions::new().create(true).append(true).open(&log_path) {
        Ok(f) => f,
        Err(e) => {
            log_io_error(format!("failed to open log for event: {e}"));
            return;
        }
    };

    if let Err(e) = writeln!(file, "{}", line) {
        log_io_error(format!("failed to write event: {e}"));
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
    efficiency: &crate::state::RunEfficiency,
) {
    let Some(_log_dir) = ensure_logs_dir() else {
        return;
    };
    let log_path = run_log_path(run_id);

    let mut summary = serde_json::json!({
        "record_type": "run_summary",
        "run_id": run_id,
        "model_id": model_id,
        "stop_reason": stop_reason,
        "total_steps": efficiency.total_steps,
        "total_model_turns": efficiency.total_model_turns,
        "parallel_tool_groups": efficiency.parallel_tool_groups,
        "cache_hits": efficiency.cache_hits,
        "cache_misses": efficiency.cache_misses,
        "duration_ms": efficiency.duration_ms,
        "exploration_turns": efficiency.exploration_turns,
        "readonly_tools_in_exploration": efficiency.readonly_tools_in_exploration,
        "serial_batch_nudges": efficiency.serial_batch_nudges,
        "no_edit_nudges": efficiency.no_edit_nudges,
        "governance_rejections": efficiency.governance_rejections,
        "repeated_actions_blocked": efficiency.repeated_actions_blocked,
        "verification_families_blocked": efficiency.verification_families_blocked,
        "review_or_finish_transitions": efficiency.review_or_finish_transitions,
        "no_progress_terminations": efficiency.no_progress_terminations,
        "turns_after_last_edit": efficiency.turns_after_last_edit,
        "governance_mode": efficiency.governance_mode,
        "timestamp": chrono::Utc::now().to_rfc3339(),
    });
    if let Some(avg) = efficiency.avg_readonly_per_exploration_turn() {
        summary["avg_readonly_per_exploration_turn"] =
            serde_json::json!((avg * 100.0).round() / 100.0);
    }
    if let Some(ms) = efficiency.time_to_first_edit_ms {
        summary["time_to_first_edit_ms"] = serde_json::json!(ms);
    }
    if let Some(ms) = efficiency.time_to_first_verification_ms {
        summary["time_to_first_verification_ms"] = serde_json::json!(ms);
    }

    let line = match serde_json::to_string(&summary) {
        Ok(s) => s,
        Err(e) => {
            log_io_error(format!("failed to serialize run summary: {e}"));
            return;
        }
    };

    let mut file = match OpenOptions::new().create(true).append(true).open(&log_path) {
        Ok(f) => f,
        Err(e) => {
            log_io_error(format!("failed to open log for summary: {e}"));
            return;
        }
    };

    if let Err(e) = writeln!(file, "{}", line) {
        log_io_error(format!("failed to write run summary: {e}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::step::Action;
    use crate::state::stop_reason::StopReason;
    use crate::tools::ToolResult;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use tempfile::tempdir;

    static TEST_LOG_LOCK: Mutex<()> = Mutex::new(());

    fn with_temp_logs<R>(f: impl FnOnce(&std::path::Path) -> R) -> R {
        let _guard = TEST_LOG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempdir().unwrap();
        set_logs_dir_override(Some(dir.path().to_path_buf()));
        let out = f(dir.path());
        set_logs_dir_override(None);
        out
    }

    fn make_test_entry(run_id: &str) -> LogEntry {
        LogEntry {
            run_id: run_id.to_string(),
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
        let entry = make_test_entry("test-run-123");
        let json = serde_json::to_string(&entry).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["run_id"], "test-run-123");
        assert_eq!(parsed["step_index"], 0);
        assert_eq!(parsed["model_id"], "llama-3.1-70b");
    }

    #[test]
    fn test_write_creates_jsonl_file() {
        with_temp_logs(|dir| {
            let entry = make_test_entry("test-write-456");
            write_entry(&entry);
            let log_path = run_log_path(&entry.run_id);
            assert!(log_path.exists());
            assert!(log_path.starts_with(dir));
            let content = std::fs::read_to_string(&log_path).unwrap();
            for line in content.lines() {
                let _: serde_json::Value = serde_json::from_str(line).unwrap();
            }
        });
    }

    #[test]
    fn test_write_appends() {
        with_temp_logs(|_| {
            let entry = make_test_entry("test-append-456");
            let _ = std::fs::remove_file(run_log_path(&entry.run_id));
            write_entry(&entry);
            write_entry(&entry);
            let content = std::fs::read_to_string(run_log_path(&entry.run_id)).unwrap();
            assert_eq!(content.lines().count(), 2);
        });
    }

    #[test]
    fn test_write_run_summary_appends() {
        with_temp_logs(|_| {
            let entry = make_test_entry("test-summary-789");
            write_entry(&entry);
            write_run_summary(
                &entry.run_id,
                &entry.model_id,
                &StopReason::TaskComplete,
                &crate::state::RunEfficiency {
                    total_steps: 1,
                    total_model_turns: 1,
                    parallel_tool_groups: 0,
                    cache_hits: 0,
                    cache_misses: 1,
                    duration_ms: 10,
                    exploration_turns: 0,
                    readonly_tools_in_exploration: 0,
                    serial_batch_nudges: 0,
                    no_edit_nudges: 0,
                    governance_rejections: 0,
                    repeated_actions_blocked: 0,
                    verification_families_blocked: 0,
                    review_or_finish_transitions: 0,
                    no_progress_terminations: 0,
                    turns_after_last_edit: 0,
                    governance_mode: "off".into(),
                    time_to_first_edit_ms: None,
                    time_to_first_verification_ms: None,
                },
            );
            let content = std::fs::read_to_string(run_log_path(&entry.run_id)).unwrap();
            assert!(content.contains("run_summary"));
        });
    }

    #[test]
    fn write_event_creates_missing_dir() {
        with_temp_logs(|dir| {
            set_logs_dir_override(Some(dir.join("nested-logs")));
            let nested = logs_dir();
            assert!(!nested.exists());
            write_event("evt-1", "smoke", "detail", 0);
            assert!(nested.exists());
            assert!(run_log_path("evt-1").exists());
        });
    }

    #[test]
    fn logs_dir_prefers_override_over_defaults() {
        with_temp_logs(|dir| {
            assert_eq!(logs_dir(), dir);
        });
    }
}
