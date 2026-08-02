use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{LazyLock, RwLock};

use serde::{Deserialize, Serialize};

use crate::state::step::{Action, Step};
use crate::tools::ToolResult;

/// Optional override for tests (`set_logs_dir_override`).
static LOGS_DIR_OVERRIDE: RwLock<Option<PathBuf>> = RwLock::new(None);

/// Per-run log path registration: `run_id` → numbered file like `3_a1b2c3d4.jsonl`.
static RUN_LOG_PATHS: LazyLock<RwLock<HashMap<String, PathBuf>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

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
    // Drop any registered paths that pointed at the old directory.
    if let Ok(mut paths) = RUN_LOG_PATHS.write() {
        paths.clear();
    }
}

/// Directory for run JSONL logs.
///
/// Priority:
/// 1. [`set_logs_dir_override`] (tests)
/// 2. `KAIRO_LOG_DIR`
/// 3. `./logs` under the process current directory (usually `kairo/logs`
///    when you `cargo run` from the crate, already gitignored)
/// 4. `$XDG_DATA_HOME/kairo/logs` only if cwd is unavailable
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
    // Unit/integration tests must not write into the developer's tree.
    #[cfg(test)]
    {
        return std::env::temp_dir().join("kairo-test-logs");
    }
    #[cfg(not(test))]
    {
        if let Ok(cwd) = std::env::current_dir() {
            return cwd.join("logs");
        }
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

/// Allocate the next ordinal run file: `1_<id8>.jsonl`, `2_<id8>.jsonl`, …
///
/// Call once at run start. Subsequent [`run_log_path`] / writes use this path.
/// Always prints the path so corpus verification does not require `--verbose`.
pub fn begin_run_log(run_id: &str) -> PathBuf {
    let Some(log_dir) = ensure_logs_dir() else {
        return logs_dir().join(format!("run_{run_id}.jsonl"));
    };

    let seq = next_run_sequence(&log_dir);
    let short = short_run_id(run_id);
    let path = log_dir.join(format!("{seq}_{short}.jsonl"));

    if let Ok(mut paths) = RUN_LOG_PATHS.write() {
        paths.insert(run_id.to_string(), path.clone());
    }

    // Always visible — quiet CLI still needs to know where the ordered log is.
    println!("[agent] log #{seq} → {}", path.display());

    let started = serde_json::json!({
        "record_type": "run_started",
        "seq": seq,
        "run_id": run_id,
        "log_file": path.file_name().and_then(|s| s.to_str()).unwrap_or(""),
        "logs_dir": log_dir.display().to_string(),
        "timestamp": chrono::Utc::now().to_rfc3339(),
    });
    append_json_line(&path, &started);

    path
}

fn short_run_id(run_id: &str) -> String {
    let alnum: String = run_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(8)
        .collect();
    if alnum.is_empty() {
        "run".into()
    } else {
        alnum
    }
}

/// Next free integer prefix in `dir` (files named `N_*.jsonl` or `N.jsonl`).
pub fn next_run_sequence(dir: &std::path::Path) -> u32 {
    let mut max = 0u32;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 1;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.ends_with(".jsonl") {
            continue;
        }
        let stem = name.trim_end_matches(".jsonl");
        let num_str = stem.split('_').next().unwrap_or(stem);
        if let Ok(n) = num_str.parse::<u32>() {
            max = max.max(n);
        }
    }
    max.saturating_add(1).max(1)
}

/// Path for one run's JSONL file (numbered after [`begin_run_log`]).
pub fn run_log_path(run_id: &str) -> PathBuf {
    if let Ok(paths) = RUN_LOG_PATHS.read() {
        if let Some(p) = paths.get(run_id) {
            return p.clone();
        }
    }
    // Fallback for tests / events before begin_run_log.
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
    if !crate::logging::is_quiet() {
        eprintln!("[logging] {}", message.as_ref());
    }
}

fn append_json_line(log_path: &std::path::Path, value: &serde_json::Value) {
    let line = match serde_json::to_string(value) {
        Ok(s) => s,
        Err(e) => {
            log_io_error(format!("failed to serialize log line: {e}"));
            return;
        }
    };
    let mut file = match OpenOptions::new().create(true).append(true).open(log_path) {
        Ok(f) => f,
        Err(e) => {
            log_io_error(format!("failed to open {}: {e}", log_path.display()));
            return;
        }
    };
    if let Err(e) = writeln!(file, "{}", line) {
        log_io_error(format!("failed to write log line: {e}"));
    }
}

/// Write a single LogEntry as one JSON line to the run's log file.
pub fn write_entry(entry: &LogEntry) {
    let Some(_log_dir) = ensure_logs_dir() else {
        return;
    };
    let log_path = run_log_path(&entry.run_id);

    let Ok(line) = serde_json::to_string(entry) else {
        log_io_error("failed to serialize log entry");
        return;
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

    append_json_line(&log_path, &event);
}

/// Print + persist a full Layer-4 / stall / recovery thinking snapshot.
///
/// Printed unconditionally (not gated by quiet) so corpus verification can
/// follow transitions without `--verbose`.
pub fn emit_thinking_state(
    run_id: &str,
    step_index: u32,
    model_turn: u32,
    tool: &str,
    payload: &serde_json::Value,
) {
    let transition = payload
        .get("transition")
        .and_then(|v| v.as_str())
        .unwrap_or("?");
    let phase = payload
        .get("phase")
        .and_then(|v| v.as_str())
        .unwrap_or("?");
    let stalls = payload
        .get("consecutive_stalls")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let stall_kind = payload
        .get("stall_kind")
        .and_then(|v| v.as_str())
        .unwrap_or("none");
    let legacy = payload
        .get("legacy_decision")
        .and_then(|v| v.as_str())
        .unwrap_or("?");
    let candidate = payload
        .get("candidate_decision")
        .and_then(|v| v.as_str())
        .unwrap_or("?");
    let candidate_reason = payload
        .get("candidate_reason")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let recovery = payload
        .get("recovery_active")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    println!("[transition] step={step_index} turn={model_turn} tool={tool} → {transition}");
    println!(
        "[state] phase={phase} stalls={stalls} kind={stall_kind} recovery_active={recovery} \
         legacy={legacy} candidate={candidate} ({candidate_reason})"
    );
    if let Some(detail) = payload.get("transition_detail").and_then(|v| v.as_str()) {
        if !detail.is_empty() {
            println!("[transition.detail] {detail}");
        }
    }

    let mut record = payload.clone();
    if let Some(obj) = record.as_object_mut() {
        obj.insert("record_type".into(), serde_json::json!("thinking_state"));
        obj.insert("run_id".into(), serde_json::json!(run_id));
        obj.insert("step_index".into(), serde_json::json!(step_index));
        obj.insert("model_turn".into(), serde_json::json!(model_turn));
        obj.insert("tool".into(), serde_json::json!(tool));
        obj.insert(
            "timestamp".into(),
            serde_json::json!(chrono::Utc::now().to_rfc3339()),
        );
    }
    let Some(_log_dir) = ensure_logs_dir() else {
        return;
    };
    append_json_line(&run_log_path(run_id), &record);
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
    write_run_summary_with_assessment(run_id, model_id, stop_reason, efficiency, None);
}

/// Write a run summary, optionally including Slice 5F completion assessment.
pub fn write_run_summary_with_assessment(
    run_id: &str,
    model_id: &str,
    stop_reason: &crate::state::stop_reason::StopReason,
    efficiency: &crate::state::RunEfficiency,
    completion_assessment: Option<&str>,
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
    if let Some(assessment) = completion_assessment {
        summary["completion_assessment"] = serde_json::json!(assessment);
    }

    append_json_line(&log_path, &summary);
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
            let _ = begin_run_log(&entry.run_id);
            write_entry(&entry);
            let log_path = run_log_path(&entry.run_id);
            assert!(log_path.exists());
            assert!(log_path.starts_with(dir));
            let name = log_path.file_name().unwrap().to_string_lossy();
            assert!(name.starts_with("1_"), "got {name}");
            let content = std::fs::read_to_string(&log_path).unwrap();
            for line in content.lines() {
                let _: serde_json::Value = serde_json::from_str(line).unwrap();
            }
        });
    }

    #[test]
    fn sequential_run_numbers_increment() {
        with_temp_logs(|dir| {
            let p1 = begin_run_log("aaaaaaaa-1111");
            let p2 = begin_run_log("bbbbbbbb-2222");
            let p3 = begin_run_log("cccccccc-3333");
            assert!(p1.file_name().unwrap().to_string_lossy().starts_with("1_"));
            assert!(p2.file_name().unwrap().to_string_lossy().starts_with("2_"));
            assert!(p3.file_name().unwrap().to_string_lossy().starts_with("3_"));
            assert_eq!(next_run_sequence(dir), 4);
        });
    }

    #[test]
    fn test_write_appends() {
        with_temp_logs(|_| {
            let entry = make_test_entry("test-append-456");
            let _ = begin_run_log(&entry.run_id);
            write_entry(&entry);
            write_entry(&entry);
            let content = std::fs::read_to_string(run_log_path(&entry.run_id)).unwrap();
            // run_started + 2 entries
            assert_eq!(content.lines().count(), 3);
        });
    }

    #[test]
    fn test_write_run_summary_appends() {
        with_temp_logs(|_| {
            let entry = make_test_entry("test-summary-789");
            let _ = begin_run_log(&entry.run_id);
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
                    ..Default::default()
                },
            );
            let content = std::fs::read_to_string(run_log_path(&entry.run_id)).unwrap();
            assert!(content.contains("run_summary"));
        });
    }

    #[test]
    fn write_event_creates_missing_dir() {
        with_temp_logs(|_| {
            let _ = begin_run_log("evt-1");
            write_event("evt-1", "smoke", "detail", 0);
            assert!(run_log_path("evt-1").exists());
        });
    }
}
