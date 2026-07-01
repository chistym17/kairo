mod config;
mod llm;
mod logging;
mod loop_engine;
mod state;
mod tools;

use llm::groq_client::GroqClient;
use state::agent_state::AgentState;

fn system_prompt() -> String {
    r#"You are a coding agent. Your job is to find and fix bugs in a code repository.

You have four tools:
- read_file: read a file's current contents
- write_file: overwrite a file with new content (provide the COMPLETE file, not just changed lines)
- run_command: run a shell command in the workspace (use this to run tests)
- finish: signal that you are done

## Rules you must follow

1. Always read a file before editing it. Never guess at its contents.
2. After editing a file, always run the tests to verify your fix worked.
3. Only call finish after you have seen the tests pass with exit code 0.
4. If the tests still fail after your edit, read the error output carefully and try again.
5. Only edit source files. Never edit test files.
6. Provide the COMPLETE file content when using write_file — not just the changed lines.
7. If you cannot fix the problem after several attempts, call finish and explain what you tried.

## Workflow

The correct workflow is always:
  read_file → understand the bug → write_file with fix → run_command to verify → finish if passing

Do not skip steps. Do not call finish before running the tests.
Do not modify test files under any circumstances."#
        .to_string()
}

#[tokio::main]
async fn main() {
    // Load .env file — must happen before Config::load()
    dotenv::dotenv().ok(); // .ok() = don't panic if .env doesn't exist

    // Parse CLI argument — the goal string
    let goal = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: agent-v1 \"<goal>\"");
        eprintln!("example: agent-v1 \"fix the failing test in test_calc.py\"");
        std::process::exit(1);
    });

    // Load config from environment
    let config = config::Config::load();

    println!("[agent-v1] starting");
    println!("[agent-v1] goal: {}", goal);
    println!("[agent-v1] workspace: {}", config.workspace_root.display());
    println!("[agent-v1] model: {}", config.model_id);
    println!("[agent-v1] max_steps: {}", config.max_steps);

    // Verify workspace exists before starting
    if !config.workspace_root.exists() {
        eprintln!(
            "[agent-v1] error: workspace_root does not exist: {}",
            config.workspace_root.display()
        );
        std::process::exit(1);
    }

    // Build the LLM client
    let llm = GroqClient::new(&config.groq_api_key, &config.model_id);

    // Build fresh agent state for this run
    let mut state = AgentState::new(
        goal,
        config.workspace_root,
        config.model_id,
        config.max_steps,
    );

    println!("[agent-v1] run_id: {}", state.run_id);
    println!("[agent-v1] log: logs/run_{}.jsonl", state.run_id);

    // Run the loop
    let stop_reason = loop_engine::run(&mut state, &llm, &system_prompt()).await;

    // Final summary
    println!("\n[agent-v1] ── run complete ──────────────────────────");
    println!("[agent-v1] stop reason: {:?}", stop_reason);
    println!("[agent-v1] total steps: {}", state.step_count);
    println!("[agent-v1] log: logs/run_{}.jsonl", state.run_id);

    // Exit code reflects success — useful for scripting and Phase 3 batch runs
    match stop_reason {
        state::stop_reason::StopReason::TaskComplete => std::process::exit(0),
        _ => std::process::exit(1),
    }
}
