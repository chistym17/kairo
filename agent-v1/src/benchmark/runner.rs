use anyhow::{Context, Result};

use crate::agent::AgentRuntime;
use crate::llm::build_client;

use super::config::BenchmarkConfig;
use super::metrics::{self, aggregate, build_trial_result, log_path_for_run, BenchmarkReport};
use super::oracle;
use super::prompt;
use super::report;
use super::run_index;
use super::task;
use super::worktree;

pub async fn run_benchmark(config: BenchmarkConfig) -> Result<BenchmarkReport> {
    let tasks = task::load_tasks(&config.tasks_path)?;
    let system_prompt = prompt::system_prompt();
    let llm = build_client(&config.agent);
    let runtime = AgentRuntime::new(
        llm,
        &config.agent.model_id,
        config.agent.max_steps,
        config.agent.step_delay_ms,
        config.agent.tool_use_max_retries,
        config.agent.tool_use_retry_delay_ms,
        system_prompt.clone(),
        config.agent.max_context_tokens,
        config.agent.context_reserve_tokens,
        config.agent.keep_recent_session_messages,
    );

    println!("[benchmark] tasks: {}", tasks.len());
    println!("[benchmark] trials per task: {}", config.trials);
    println!("[benchmark] model: {}", config.agent.model_id);
    println!("[benchmark] output: {}", config.output_dir.display());

    let mut trial_results = Vec::new();

    for task in &tasks {
        let git_ref = task.resolved_git_ref();

        for trial in 0..config.trials {
            println!(
                "\n[benchmark] ── {} (trial {}) ─────────────────────",
                task.id, trial
            );

            worktree::reset_worktree(&config.repo_dir, &git_ref, &task.workspace).with_context(
                || format!("failed to reset worktree for {} at {}", task.id, git_ref),
            )?;

            let run = runtime
                .run(task.goal.clone(), task.workspace.clone(), None, None)
                .await
                .with_context(|| format!("agent runtime failed for task {}", task.id))?;

            run_index::record_run(&config.output_dir, &run.run_id, &task.id, trial)?;

            println!("[benchmark] run_id: {}", run.run_id);
            println!("[benchmark] workspace: {}", task.workspace.display());

            let (oracle, readonly_violations) = oracle::run_oracle_strict(
                &task.workspace,
                &task.verify_cmd,
                &task.files_readonly,
            )?;
            if !readonly_violations.is_empty() {
                println!("[benchmark] readonly violation: {:?}", readonly_violations);
            }
            let parsed = metrics::parse_run_log(&log_path_for_run(&run.run_id))?;

            if parsed.edited_test_file {
                println!("[benchmark] warning: run edited a test file");
            }

            let diff_preview = match worktree::diff_preview(&task.workspace, 60) {
                Ok(s) if s.trim().is_empty() => None,
                Ok(s) => Some(s),
                Err(e) => Some(format!("(failed to compute diff preview: {e:#})")),
            };

            let trial_result = build_trial_result(
                &task.id,
                task.tier,
                trial,
                &run.run_id,
                &parsed,
                oracle.passed,
                &oracle.output,
                diff_preview,
            );

            println!(
                "[benchmark] result: oracle={} benchmark={} steps={}",
                if trial_result.oracle_passed { "pass" } else { "fail" },
                if trial_result.benchmark_passed {
                    "pass"
                } else {
                    "fail"
                },
                trial_result.total_steps
            );

            trial_results.push(trial_result);
        }
    }

    let report = aggregate(trial_results);
    report::print_summary(&report);
    report::write_reports(&config.output_dir, &report)?;

    Ok(report)
}
