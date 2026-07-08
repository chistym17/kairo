use anyhow::{Context, Result};

use crate::llm::groq_client::GroqClient;
use crate::loop_engine;
use crate::state::agent_state::AgentState;

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
    let llm = GroqClient::new(&config.agent.groq_api_key, &config.agent.model_id);
    let system_prompt = prompt::system_prompt();

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

            let mut state = AgentState::new(
                &task.goal,
                &task.workspace,
                &config.agent.model_id,
                config.agent.max_steps,
            );

            run_index::record_run(&config.output_dir, &state.run_id, &task.id, trial)?;

            println!("[benchmark] run_id: {}", state.run_id);
            println!("[benchmark] workspace: {}", task.workspace.display());

            let _stop_reason = loop_engine::run(
                &mut state,
                &llm,
                &system_prompt,
                config.agent.step_delay_ms,
                config.agent.tool_use_max_retries,
                config.agent.tool_use_retry_delay_ms,
            )
            .await;

            let oracle = oracle::run_oracle(&task.workspace, &task.verify_cmd)?;
            let parsed = metrics::parse_run_log(&log_path_for_run(&state.run_id))?;

            if parsed.edited_test_file {
                println!("[benchmark] warning: run edited a test file");
            }

            let trial_result = build_trial_result(
                &task.id,
                task.tier,
                trial,
                &state.run_id,
                &parsed,
                oracle.passed,
                &oracle.output,
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
