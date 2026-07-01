use crate::llm::{LlmClient, LlmError};
use crate::logging::{write_entry, write_run_summary, LogEntry};
use crate::state::agent_state::AgentState;
use crate::state::step::Step;
use crate::state::stop_reason::StopReason;
use crate::tools;

/// Run the agent loop until a stop condition is reached.
///
/// This function is the entire agent in one place:
///   goal → LLM → tool call → execute → log → feed back → repeat
///
/// It never changes shape across phases:
///   - Phase 2: tools::dispatch() handles ApplyPatch internally
///   - Phase 3: caller wraps this in a benchmark loop, passing
///     different AgentState per task
///   - Phase 4: caller constructs AgentState with a worktree workspace_root
///
/// Everything that changes lives in what gets passed in, not in here.
pub async fn run(state: &mut AgentState, llm: &dyn LlmClient, system_prompt: &str) -> StopReason {
    println!(
        "\n[agent] starting run {}\n[agent] goal: {}\n[agent] model: {}\n[agent] max_steps: {}\n",
        state.run_id, state.goal, state.model_id, state.max_steps
    );

    loop {
        // ── 1. Check step limit before asking the LLM ──────────────────
        if state.is_exhausted() {
            println!(
                "\n[agent] max steps ({}) reached — stopping",
                state.max_steps
            );
            let reason = StopReason::MaxStepsExceeded;
            write_run_summary(&state.run_id, &state.model_id, &reason, state.step_count);
            return reason;
        }

        println!(
            "\n[agent] ── step {} ──────────────────────────────",
            state.step_count
        );

        // ── 2. Ask the LLM what to do next ─────────────────────────────
        let action = match llm
            .next_action(system_prompt, &state.goal, &state.history)
            .await
        {
            Ok(action) => {
                println!("[agent] action: {:?}", action);
                action
            }

            // Model returned text instead of a tool call
            Err(LlmError::NoToolCall(text)) => {
                println!(
                    "[agent] model returned text instead of tool call:\n{}",
                    &text[..text.len().min(300)]
                );
                // Don't stop — this can happen transiently with smaller models.
                // Increment step count so we don't loop forever, then retry.
                // The full history gives the model context to self-correct.
                state.step_count += 1;
                continue;
            }

            // Malformed tool call — log and retry
            Err(LlmError::MalformedToolCall(msg)) => {
                println!("[agent] malformed tool call: {}", msg);
                state.step_count += 1;
                continue;
            }

            // Rate limited — wait and retry
            Err(LlmError::RateLimited) => {
                println!("[agent] rate limited — waiting 10s");
                tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
                // Don't increment step count for rate limit retries
                continue;
            }

            // Hard API error — stop the run
            Err(LlmError::ApiError(msg)) => {
                println!("[agent] API error: {}", msg);
                let reason = StopReason::ToolError;
                write_run_summary(&state.run_id, &state.model_id, &reason, state.step_count);
                return reason;
            }
        };

        // ── 3. Execute the tool ─────────────────────────────────────────
        let result = tools::dispatch(&action, &state.workspace_root);

        println!(
            "[agent] result: success={} output_len={}{}",
            result.success,
            result.output.len(),
            result
                .error
                .as_ref()
                .map(|e| format!(" error={}", e))
                .unwrap_or_default()
        );

        // If there's a diff, show a preview in the terminal
        if let Some(diff) = &result.diff {
            if !diff.is_empty() {
                let preview: String = diff.lines().take(10).collect::<Vec<_>>().join("\n");
                println!("[agent] diff preview:\n{}", preview);
                if diff.lines().count() > 10 {
                    println!("[agent] ... ({} total diff lines)", diff.lines().count());
                }
            }
        }

        // ── 4. Check if this action should stop the loop ────────────────
        // Capture stop reason before consuming result into Step
        let stop_reason = result.stop_reason.clone();

        // ── 5. Build the step record ────────────────────────────────────
        let step = Step::new(state.step_count, action, result);

        // ── 6. Write to log immediately — before updating state ─────────
        // If we crash between here and record_step(), we lose the in-memory
        // history entry but keep the log entry. Acceptable tradeoff.
        let entry = LogEntry::from_step(&state.run_id, &state.model_id, &step);
        write_entry(&entry);

        // ── 7. Push to history so LLM sees it next turn ─────────────────
        state.record_step(step);

        // ── 8. Stop if the action signalled termination ─────────────────
        if let Some(reason) = stop_reason {
            println!("\n[agent] stopping: {:?}", reason);
            write_run_summary(&state.run_id, &state.model_id, &reason, state.step_count);
            return reason;
        }
    }
}
