use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::logging::LogEntry;
use crate::state::step::Action;
use crate::state::stop_reason::StopReason;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureCategory {
    WrongFile,
    TestGaming,
    NoSourceEdit,
    SyntaxError,
    PatchFailed,
    MaxSteps,
    GaveUp,
    ApiError,
    FalseSuccess,
    PromptConflict,
    Other,
}

impl FailureCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::WrongFile => "wrong_file",
            Self::TestGaming => "test_gaming",
            Self::NoSourceEdit => "no_source_edit",
            Self::SyntaxError => "syntax_error",
            Self::PatchFailed => "patch_failed",
            Self::MaxSteps => "max_steps",
            Self::GaveUp => "gave_up",
            Self::ApiError => "api_error",
            Self::FalseSuccess => "false_success",
            Self::PromptConflict => "prompt_conflict",
            Self::Other => "other",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunSummary {
    pub stop_reason: StopReason,
    pub total_steps: u32,
    pub model_id: String,
    pub timestamp: String,
}

#[derive(Debug, Clone)]
pub struct ParsedRunLog {
    pub summary: Option<RunSummary>,
    pub first_timestamp: Option<String>,
    pub last_timestamp: Option<String>,
    pub edited_test_file: bool,
    pub patch_failed: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct TrialResult {
    pub task_id: String,
    pub tier: u32,
    pub trial: u32,
    pub run_id: String,
    pub model_id: String,
    pub stop_reason: StopReason,
    pub total_steps: u32,
    pub oracle_passed: bool,
    pub benchmark_passed: bool,
    pub failure_category: Option<FailureCategory>,
    pub duration_secs: Option<f64>,
    pub edited_test_file: bool,
    pub diff_preview: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BenchmarkReport {
    pub total_runs: u32,
    pub benchmark_passed: u32,
    pub pass_rate: f64,
    pub avg_steps_on_success: Option<f64>,
    pub failure_counts: Vec<(String, u32)>,
    pub trials: Vec<TrialResult>,
    pub analysis: String,
}

pub fn parse_run_log(path: &Path) -> Result<ParsedRunLog> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("failed to read log {}", path.display()))?;

    let mut summary = None;
    let mut first_timestamp = None;
    let mut last_timestamp = None;
    let mut edited_test_file = false;
    let mut patch_failed = false;

    for line in content.lines().filter(|l| !l.trim().is_empty()) {
        let value: serde_json::Value =
            serde_json::from_str(line).context("invalid JSON in log line")?;

        if value.get("record_type") == Some(&serde_json::Value::String("run_summary".into())) {
            summary = Some(RunSummary {
                stop_reason: serde_json::from_value(value["stop_reason"].clone())?,
                total_steps: value["total_steps"]
                    .as_u64()
                    .context("run_summary missing total_steps")? as u32,
                model_id: value["model_id"]
                    .as_str()
                    .context("run_summary missing model_id")?
                    .to_string(),
                timestamp: value["timestamp"]
                    .as_str()
                    .context("run_summary missing timestamp")?
                    .to_string(),
            });
            last_timestamp = summary.as_ref().map(|s| s.timestamp.clone());
            continue;
        }

        if value.get("record_type").is_some() {
            if let Some(ts) = value["timestamp"].as_str() {
                if first_timestamp.is_none() {
                    first_timestamp = Some(ts.to_string());
                }
                last_timestamp = Some(ts.to_string());
            }
            continue;
        }

        let entry: LogEntry = serde_json::from_value(value)?;

        if first_timestamp.is_none() {
            first_timestamp = Some(entry.timestamp.clone());
        }
        last_timestamp = Some(entry.timestamp.clone());

        // Only count *edits*, not reads.
        if is_mutating_action(&entry.action) {
            let path_str = action_path(&entry.action);
            if path_str.contains("test_") {
                edited_test_file = true;
            }
        }

        if tool_name(&entry.action) == "apply_patch" && !entry.result.success {
            patch_failed = true;
        }
    }

    Ok(ParsedRunLog {
        summary,
        first_timestamp,
        last_timestamp,
        edited_test_file,
        patch_failed,
    })
}

pub fn build_trial_result(
    task_id: &str,
    tier: u32,
    trial: u32,
    run_id: &str,
    parsed: &ParsedRunLog,
    oracle_passed: bool,
    oracle_output: &str,
    diff_preview: Option<String>,
) -> TrialResult {
    let summary = parsed.summary.clone().unwrap_or(RunSummary {
        stop_reason: StopReason::ToolError,
        total_steps: 0,
        model_id: "unknown".to_string(),
        timestamp: String::new(),
    });

    let has_diff = diff_preview
        .as_ref()
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);

    let benchmark_passed =
        oracle_passed && summary.stop_reason == StopReason::TaskComplete && has_diff;

    let failure_category = if benchmark_passed {
        None
    } else {
        Some(classify_failure(
            &summary,
            oracle_passed,
            parsed,
            oracle_output,
            has_diff,
        ))
    };

    TrialResult {
        task_id: task_id.to_string(),
        tier,
        trial,
        run_id: run_id.to_string(),
        model_id: summary.model_id.clone(),
        stop_reason: summary.stop_reason.clone(),
        total_steps: summary.total_steps,
        oracle_passed,
        benchmark_passed,
        failure_category,
        duration_secs: duration_between(&parsed.first_timestamp, &parsed.last_timestamp),
        edited_test_file: parsed.edited_test_file,
        diff_preview,
    }
}

pub fn aggregate(trials: Vec<TrialResult>) -> BenchmarkReport {
    let total_runs = trials.len() as u32;
    let benchmark_passed = trials.iter().filter(|t| t.benchmark_passed).count() as u32;
    let pass_rate = if total_runs == 0 {
        0.0
    } else {
        benchmark_passed as f64 / total_runs as f64
    };

    let successful_steps: Vec<u32> = trials
        .iter()
        .filter(|t| t.benchmark_passed)
        .map(|t| t.total_steps)
        .collect();

    let avg_steps_on_success = if successful_steps.is_empty() {
        None
    } else {
        Some(
            successful_steps.iter().map(|s| *s as f64).sum::<f64>()
                / successful_steps.len() as f64,
        )
    };

    let mut failure_counts = std::collections::BTreeMap::new();
    for trial in &trials {
        if let Some(category) = trial.failure_category {
            *failure_counts
                .entry(category.as_str().to_string())
                .or_insert(0) += 1;
        }
    }

    let failure_counts: Vec<(String, u32)> = failure_counts.into_iter().collect();
    let analysis = write_analysis(pass_rate, benchmark_passed, total_runs, &failure_counts);

    BenchmarkReport {
        total_runs,
        benchmark_passed,
        pass_rate,
        avg_steps_on_success,
        failure_counts,
        trials,
        analysis,
    }
}

pub fn log_path_for_run(run_id: &str) -> PathBuf {
    PathBuf::from("logs").join(format!("run_{run_id}.jsonl"))
}

fn classify_failure(
    summary: &RunSummary,
    oracle_passed: bool,
    parsed: &ParsedRunLog,
    oracle_output: &str,
    has_diff: bool,
) -> FailureCategory {
    if oracle_output.contains("[benchmark] readonly file violation") {
        return FailureCategory::TestGaming;
    }

    if oracle_passed && summary.stop_reason == StopReason::TaskComplete && !has_diff {
        return FailureCategory::NoSourceEdit;
    }

    if parsed.edited_test_file {
        return FailureCategory::WrongFile;
    }

    if parsed.patch_failed {
        return FailureCategory::PatchFailed;
    }

    match summary.stop_reason {
        StopReason::MaxStepsExceeded => return FailureCategory::MaxSteps,
        StopReason::ToolError if summary.total_steps <= 1 => return FailureCategory::ApiError,
        StopReason::TaskComplete | StopReason::CompletedUnverified if !oracle_passed => {
            return FailureCategory::FalseSuccess
        }
        StopReason::ModelGaveUp if !oracle_passed => return FailureCategory::GaveUp,
        _ => {}
    }

    if summary.stop_reason == StopReason::TaskComplete && summary.total_steps <= 1 {
        return FailureCategory::PromptConflict;
    }

    if !oracle_passed
        && (oracle_output.contains("SyntaxError")
            || oracle_output.contains("ImportError")
            || oracle_output.contains("ERROR collecting"))
    {
        return FailureCategory::SyntaxError;
    }

    if !oracle_passed {
        return FailureCategory::GaveUp;
    }

    FailureCategory::Other
}

fn duration_between(start: &Option<String>, end: &Option<String>) -> Option<f64> {
    let start = chrono::DateTime::parse_from_rfc3339(start.as_ref()?).ok()?;
    let end = chrono::DateTime::parse_from_rfc3339(end.as_ref()?).ok()?;
    Some((end - start).num_milliseconds() as f64 / 1000.0)
}

fn write_analysis(
    pass_rate: f64,
    passed: u32,
    total: u32,
    failure_counts: &[(String, u32)],
) -> String {
    let pct = pass_rate * 100.0;
    let mut analysis = format!(
        "Benchmark completed {passed}/{total} runs ({pct:.0}% pass rate). "
    );

    if failure_counts.is_empty() {
        analysis.push_str("No failures recorded.");
        return analysis;
    }

    analysis.push_str("Failures by category: ");
    let parts: Vec<String> = failure_counts
        .iter()
        .map(|(name, count)| format!("{name}={count}"))
        .collect();
    analysis.push_str(&parts.join(", "));
    analysis.push('.');

    if let Some((top, count)) = failure_counts.first() {
        analysis.push_str(&format!(
            " The most common failure was {top} ({count} run(s))."
        ));
    }

    analysis
}

fn tool_name(action: &Action) -> &'static str {
    match action {
        Action::ReadFile { .. } => "read_file",
        Action::ListFiles { .. } => "list_files",
        Action::SearchCode { .. } => "search_code",
        Action::FindFiles { .. } => "find_files",
        Action::DetectProjectSnapshot { .. } => "detect_project_snapshot",
        Action::GitStatus => "git_status",
        Action::GitDiff { .. } => "git_diff",
        Action::WriteFile { .. } => "write_file",
        Action::ApplyPatch { .. } => "apply_patch",
        Action::RunCommand { .. } => "run_command",
        Action::RunShell { .. } => "run_shell",
        Action::UpdatePlan { .. } => "update_plan",
        Action::Finish { .. } => "finish",
    }
}

fn action_path(action: &Action) -> String {
    match action {
        Action::ReadFile { path, .. }
        | Action::ListFiles { path }
        | Action::SearchCode { path, .. }
        | Action::FindFiles { path, .. }
        | Action::DetectProjectSnapshot { path }
        | Action::WriteFile { path, .. }
        | Action::ApplyPatch { path, .. } => path.to_string_lossy().to_string(),
        Action::RunCommand { cwd, .. } | Action::RunShell { cwd, .. } => {
            cwd.to_string_lossy().to_string()
        }
        Action::GitDiff { path } => path
            .as_ref()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default(),
        Action::GitStatus | Action::UpdatePlan { .. } | Action::Finish { .. } => String::new(),
    }
}

fn is_mutating_action(action: &Action) -> bool {
    matches!(action, Action::ApplyPatch { .. } | Action::WriteFile { .. })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::ToolResult;
    use std::path::PathBuf;

    #[test]
    fn detects_test_file_edits() {
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("run_test.jsonl");

        let entry = LogEntry {
            run_id: "test".to_string(),
            model_id: "model".to_string(),
            step_index: 0,
            timestamp: "2026-01-01T00:00:00+00:00".to_string(),
            action: Action::ApplyPatch {
                path: PathBuf::from("test_calc.py"),
                diff: String::new(),
            },
            result: ToolResult {
                success: true,
                output: String::new(),
                diff: None,
                error: None,
                stop_reason: None,
            },
        };

        let line = serde_json::to_string(&entry).unwrap();
        let summary = serde_json::json!({
            "record_type": "run_summary",
            "run_id": "test",
            "model_id": "model",
            "stop_reason": "TaskComplete",
            "total_steps": 1,
            "timestamp": "2026-01-01T00:00:01+00:00"
        });
        fs::write(&log_path, format!("{line}\n{summary}\n")).unwrap();

        let parsed = parse_run_log(&log_path).unwrap();
        assert!(parsed.edited_test_file);
    }
}
