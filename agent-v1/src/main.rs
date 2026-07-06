use agent_v1::config;
use agent_v1::llm::groq_client::GroqClient;
use agent_v1::loop_engine;
use agent_v1::state::agent_state::AgentState;
use agent_v1::state::stop_reason::StopReason;

fn system_prompt() -> String {
    r#"You are a coding agent. Your job is to fix a bug in calc.py so that pytest tests pass.

The user's message describes which test is failing and which function to fix. Follow that — do not guess a different function.

## EXACT STEPS TO FOLLOW

Step 1: Call read_file with path "calc.py" to see the current code.
Step 2: Read test_calc.py if needed to understand what the tests expect.
Step 3: Identify the bug in calc.py only.
Step 4: Call apply_patch with path "calc.py" and a unified diff of only the lines that need to change.
Step 5: Call run_command with cmd "pytest test_calc.py" to verify.
Step 6: If tests pass, call finish. If tests still fail, go back to step 1.

## HARD RULES

RULE 1: Your FIRST action must be read_file("calc.py"). Not run_command. Not apply_patch. read_file first.

RULE 2: Never call run_command twice in a row. If pytest just failed, your next action must be read_file or apply_patch.

RULE 3: Use apply_patch to edit existing files. Use write_file only to create new files. Never rewrite the whole file when a patch will do.

RULE 4: Never edit test_calc.py. Only edit calc.py. "Fix the failing test" means fix the source code so the test passes — not edit the test file.

RULE 5: Never call finish unless the last run_command showed pytest passing with 0 failures."#
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
    println!("[agent-v1] step_delay: {}ms", config.step_delay_ms);

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
    let stop_reason = loop_engine::run(&mut state, &llm, &system_prompt(), config.step_delay_ms).await;

    // Final summary
    println!("\n[agent-v1] ── run complete ──────────────────────────");
    println!("[agent-v1] stop reason: {:?}", stop_reason);
    println!("[agent-v1] total steps: {}", state.step_count);
    println!("[agent-v1] log: logs/run_{}.jsonl", state.run_id);

    // Exit code reflects success — useful for scripting and Phase 3 batch runs
    match stop_reason {
        StopReason::TaskComplete => std::process::exit(0),
        _ => std::process::exit(1),
    }
}
