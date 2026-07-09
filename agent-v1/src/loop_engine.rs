use crate::context::ContextBuilder;
use crate::llm::{LlmClient, LlmError};
use crate::logging::{write_entry, write_event, write_run_summary, LogEntry};
use crate::session::Message;
use crate::state::agent_state::AgentState;
use crate::state::step::Step;
use crate::state::stop_reason::StopReason;
use crate::tools;
use crate::agent::{AgentEvent, AgentObserver};

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
pub async fn run(
    state: &mut AgentState,
    llm: &dyn LlmClient,
    context_builder: &ContextBuilder,
    step_delay_ms: u64,
    tool_use_max_retries: u32,
    tool_use_retry_delay_ms: u64,
    observer: Option<&dyn AgentObserver>,
    session_messages: &[Message],
) -> StopReason {
    println!(
        "\n[agent] starting run {}\n[agent] goal: {}\n[agent] model: {}\n[agent] max_steps: {}\n[agent] step_delay: {}ms\n",
        state.run_id, state.goal, state.model_id, state.max_steps, step_delay_ms
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

        emit(
            observer,
            AgentEvent::StepStarted {
                run_id: state.run_id.clone(),
                step_index: state.step_count,
            },
        );

        // ── 1.5. Throttle: wait before hitting the API ─────────────────
        if step_delay_ms > 0 && state.step_count > 0 {
            println!("[agent] waiting {}ms before next API call", step_delay_ms);
            write_event(
                &state.run_id,
                "step_delay",
                &format!("waiting {}ms before step {}", step_delay_ms, state.step_count),
                step_delay_ms,
            );
            tokio::time::sleep(tokio::time::Duration::from_millis(step_delay_ms)).await;
        }

        // ── 2. Ask the LLM what to do next ─────────────────────────────
        let action = 'next_action: loop {
            let request = context_builder.build(
                &state.workspace_root,
                &state.goal,
                session_messages,
                &state.history,
            );

            match llm.next_action(&request).await
            {
                Ok(action) => {
                    println!("[agent] action: {:?}", action);
                    break 'next_action action;
                }

                Err(LlmError::NoToolCall(text)) => {
                    println!(
                        "[agent] model returned text instead of tool call:\n{}",
                        &text[..text.len().min(300)]
                    );
                    write_event(
                        &state.run_id,
                        "no_tool_call",
                        &text[..text.len().min(200)],
                        0,
                    );
                    state.step_count += 1;
                    emit(
                        observer,
                        AgentEvent::Error {
                            run_id: state.run_id.clone(),
                            step_index: Some(state.step_count),
                            message: "model returned text instead of tool call".to_string(),
                        },
                    );
                    continue;
                }

                Err(LlmError::MalformedToolCall(msg)) => {
                    println!("[agent] malformed tool call: {}", msg);
                    write_event(&state.run_id, "malformed_tool_call", &msg, 0);
                    state.step_count += 1;
                    emit(
                        observer,
                        AgentEvent::Error {
                            run_id: state.run_id.clone(),
                            step_index: Some(state.step_count),
                            message: format!("malformed tool call: {}", msg),
                        },
                    );
                    continue;
                }

                Err(LlmError::RateLimited) => {
                    let wait_ms: u64 = 15_000;
                    println!("[agent] rate limited — waiting {}ms", wait_ms);
                    write_event(
                        &state.run_id,
                        "rate_limited",
                        &format!("API returned 429, backing off {}ms", wait_ms),
                        wait_ms,
                    );
                    tokio::time::sleep(tokio::time::Duration::from_millis(wait_ms)).await;
                    continue 'next_action;
                }

                Err(LlmError::ToolUseFailed(mut msg)) => {
                    if tool_use_max_retries == 0 {
                        println!("[agent] tool_use_failed — retries disabled, stopping");
                        write_event(
                            &state.run_id,
                            "tool_use_failed",
                            &msg[..msg.len().min(500)],
                            0,
                        );
                        let reason = StopReason::ToolError;
                        write_run_summary(
                            &state.run_id,
                            &state.model_id,
                            &reason,
                            state.step_count,
                        );
                        return reason;
                    }

                    for attempt in 1..=tool_use_max_retries {
                        println!(
                            "[agent] tool_use_failed — retry {}/{} in {}ms",
                            attempt, tool_use_max_retries, tool_use_retry_delay_ms
                        );
                        write_event(
                            &state.run_id,
                            "tool_use_failed_retry",
                            &format!(
                                "attempt {}/{}, waiting {}ms — {}",
                                attempt,
                                tool_use_max_retries,
                                tool_use_retry_delay_ms,
                                &msg[..msg.len().min(200)]
                            ),
                            tool_use_retry_delay_ms,
                        );
                        tokio::time::sleep(tokio::time::Duration::from_millis(
                            tool_use_retry_delay_ms,
                        ))
                        .await;

                        let request = context_builder.build(
                            &state.workspace_root,
                            &state.goal,
                            session_messages,
                            &state.history,
                        );

                        match llm.next_action(&request).await
                        {
                            Ok(action) => {
                                println!(
                                    "[agent] action (after tool_use_failed retry {}): {:?}",
                                    attempt, action
                                );
                                break 'next_action action;
                            }
                            Err(LlmError::ToolUseFailed(retry_msg)) => {
                                msg = retry_msg;
                                if attempt == tool_use_max_retries {
                                    println!(
                                        "[agent] tool_use_failed — exhausted {} retries, stopping",
                                        tool_use_max_retries
                                    );
                                    write_event(
                                        &state.run_id,
                                        "tool_use_failed",
                                        &msg[..msg.len().min(500)],
                                        0,
                                    );
                                    let reason = StopReason::ToolError;
                                    write_run_summary(
                                        &state.run_id,
                                        &state.model_id,
                                        &reason,
                                        state.step_count,
                                    );
                                    return reason;
                                }
                            }
                            Err(LlmError::RateLimited) => {
                                let wait_ms: u64 = 15_000;
                                println!("[agent] rate limited — waiting {}ms", wait_ms);
                                write_event(
                                    &state.run_id,
                                    "rate_limited",
                                    &format!("API returned 429, backing off {}ms", wait_ms),
                                    wait_ms,
                                );
                                tokio::time::sleep(tokio::time::Duration::from_millis(wait_ms))
                                    .await;
                            }
                            Err(other) => {
                                emit(
                                    observer,
                                    AgentEvent::Error {
                                        run_id: state.run_id.clone(),
                                        step_index: Some(state.step_count),
                                        message: other.to_string(),
                                    },
                                );
                                return handle_fatal_llm_error(state, other);
                            }
                        }
                    }
                }

                Err(LlmError::ApiError(msg)) => {
                    emit(
                        observer,
                        AgentEvent::Error {
                            run_id: state.run_id.clone(),
                            step_index: Some(state.step_count),
                            message: format!("api error: {}", msg),
                        },
                    );
                    return handle_fatal_llm_error(state, LlmError::ApiError(msg));
                }
            }
        };

        // ── 3. Execute the tool ─────────────────────────────────────────
        emit(
            observer,
            AgentEvent::ToolStarted {
                run_id: state.run_id.clone(),
                step_index: state.step_count,
                tool: tool_name(&action).to_string(),
            },
        );

        let result = tools::dispatch(&action, &state.workspace_root);

        emit(
            observer,
            AgentEvent::ToolCompleted {
                run_id: state.run_id.clone(),
                step_index: state.step_count,
                tool: tool_name(&action).to_string(),
                success: result.success,
            },
        );

        if result.success {
            if let Some(path) = mutating_path(&action) {
                emit(
                    observer,
                    AgentEvent::FileModified {
                        run_id: state.run_id.clone(),
                        step_index: state.step_count,
                        path,
                    },
                );
            }
        } else {
            emit(
                observer,
                AgentEvent::Error {
                    run_id: state.run_id.clone(),
                    step_index: Some(state.step_count),
                    message: result
                        .error
                        .clone()
                        .unwrap_or_else(|| "tool failed".to_string()),
                },
            );
        }

        if let crate::state::step::Action::RunCommand { cmd } = &action {
            emit(
                observer,
                AgentEvent::CommandCompleted {
                    run_id: state.run_id.clone(),
                    step_index: state.step_count,
                    cmd: cmd.clone(),
                    success: result.success,
                },
            );
        }

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

        emit(
            observer,
            AgentEvent::StepCompleted {
                run_id: state.run_id.clone(),
                step_index: state.step_count - 1,
                stop_reason: stop_reason.as_ref().map(|r| format!("{:?}", r)),
            },
        );

        // ── 8. Stop if the action signalled termination ─────────────────
        if let Some(reason) = stop_reason {
            println!("\n[agent] stopping: {:?}", reason);
            write_run_summary(&state.run_id, &state.model_id, &reason, state.step_count);
            return reason;
        }
    }
}

fn emit(observer: Option<&dyn AgentObserver>, event: AgentEvent) {
    if let Some(obs) = observer {
        obs.on_event(event);
    }
}

fn tool_name(action: &crate::state::step::Action) -> &'static str {
    use crate::state::step::Action;
    match action {
        Action::ReadFile { .. } => "read_file",
        Action::WriteFile { .. } => "write_file",
        Action::ApplyPatch { .. } => "apply_patch",
        Action::RunCommand { .. } => "run_command",
        Action::Finish { .. } => "finish",
    }
}

fn mutating_path(action: &crate::state::step::Action) -> Option<String> {
    use crate::state::step::Action;
    match action {
        Action::ApplyPatch { path, .. } | Action::WriteFile { path, .. } => {
            Some(path.to_string_lossy().to_string())
        }
        _ => None,
    }
}

fn handle_fatal_llm_error(state: &AgentState, err: LlmError) -> StopReason {
    match err {
        LlmError::ApiError(msg) => {
            println!("[agent] API error: {}", msg);
            write_event(&state.run_id, "api_error", &msg, 0);
        }
        other => {
            println!("[agent] unrecoverable LLM error: {}", other);
            write_event(
                &state.run_id,
                "llm_error",
                &other.to_string(),
                0,
            );
        }
    }

    let reason = StopReason::ToolError;
    write_run_summary(
        &state.run_id,
        &state.model_id,
        &reason,
        state.step_count,
    );
    reason
}
