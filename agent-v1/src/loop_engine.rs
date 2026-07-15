use crate::context::{ContextBuilder, ContextManager};
use crate::llm::{LlmClient, LlmError, LlmToolCall};
use crate::logging::{write_entry, write_event, write_run_summary, LogEntry};
use crate::session::Session;
use crate::state::agent_state::AgentState;
use crate::state::step::{Action, Step};
use crate::state::stop_reason::StopReason;
use crate::tools;
use crate::tools::ToolResult;
use crate::agent::{AgentEvent, AgentObserver};
use serde_json::json;

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
    context_manager: &ContextManager,
    step_delay_ms: u64,
    tool_use_max_retries: u32,
    tool_use_retry_delay_ms: u64,
    require_verified_finish: bool,
    observer: Option<&dyn AgentObserver>,
    mut session: Option<&mut Session>,
) -> StopReason {
    println!(
        "\n[agent] starting run {}\n[agent] goal: {}\n[agent] model: {}\n[agent] max_steps: {}\n",
        state.run_id, state.goal, state.model_id, state.max_steps
    );
    if step_delay_ms > 0 {
        println!("[agent] step_delay: {}ms (opt-in pacing)", step_delay_ms);
    }

    // Orientation once per run (no step cost). Refreshable via detect_project_snapshot.
    if state.project_snapshot.is_none() {
        let snap = tools::project_snapshot::detect(&state.workspace_root);
        println!(
            "[agent] project snapshot: {} unit(s){}",
            snap.units.len(),
            if snap.truncated { " (truncated)" } else { "" }
        );
        state.project_snapshot = Some(snap);
    }

    loop {
        // ── 1. Check step limit before asking the LLM ──────────────────
        if state.is_exhausted() {
            println!(
                "\n[agent] max model turns ({}) reached — stopping ({} tool calls)",
                state.max_steps, state.step_count
            );
            let reason = StopReason::MaxStepsExceeded;
            write_summary(state, &reason);
            return reason;
        }

        println!(
            "\n[agent] ── model turn {} (tool calls so far: {}) ──────────",
            state.model_turns, state.step_count
        );

        // ── 1.5. Throttle: wait before hitting the API ─────────────────
        if step_delay_ms > 0 && state.model_turns > 0 {
            println!("[agent] waiting {}ms before next API call", step_delay_ms);
            write_event(
                &state.run_id,
                "step_delay",
                &format!(
                    "waiting {}ms before model turn {}",
                    step_delay_ms, state.model_turns
                ),
                step_delay_ms,
            );
            tokio::time::sleep(tokio::time::Duration::from_millis(step_delay_ms)).await;
        }

        // ── 2. Ask the LLM (one turn → zero or more tool calls) ────────
        let tool_calls = 'next_actions: loop {
            let session_messages_snapshot: Vec<crate::session::Message> = session
                .as_ref()
                .map(|s| s.messages.clone())
                .unwrap_or_default();
            let goal_for_llm = goal_with_plan(state);
            let (request, full_stats, bounded_stats, reduction) =
                context_manager.build_bounded_request(
                    context_builder,
                    &state.workspace_root,
                    &goal_for_llm,
                    session.as_deref_mut(),
                    &session_messages_snapshot,
                    &state.history,
                );

            write_event(&state.run_id, "context_stats", &bounded_stats.to_json_string(), 0);
            if reduction.trimmed {
                let detail = json!({
                    "budget_tokens": reduction.budget_tokens,
                    "dropped_session_messages": reduction.dropped_session_messages,
                    "full_estimated_tokens": full_stats.estimated_tokens,
                    "bounded_estimated_tokens": bounded_stats.estimated_tokens
                })
                .to_string();
                write_event(&state.run_id, "context_reduction", &detail, 0);
            }

            match llm.next_actions(&request).await {
                Ok(calls) if calls.is_empty() => {
                    let msg = "model returned empty tool_calls".to_string();
                    println!("[agent] {}", msg);
                    write_event(&state.run_id, "malformed_tool_call", &msg, 0);
                    state.model_turns += 1;
                    emit(
                        observer,
                        AgentEvent::Error {
                            run_id: state.run_id.clone(),
                            step_index: Some(state.step_count),
                            message: msg,
                        },
                    );
                    continue;
                }
                Ok(calls) => {
                    println!(
                        "[agent] {} tool call(s): {}",
                        calls.len(),
                        calls
                            .iter()
                            .map(|c| tool_name(&c.action))
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                    break 'next_actions calls;
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
                    state.model_turns += 1;
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
                    state.model_turns += 1;
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
                    continue 'next_actions;
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
                        write_summary(state, &reason);
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

                        let session_messages_snapshot: Vec<crate::session::Message> = session
                            .as_ref()
                            .map(|s| s.messages.clone())
                            .unwrap_or_default();
                        let goal_for_llm = goal_with_plan(state);
                        let (request, full_stats, bounded_stats, reduction) =
                            context_manager.build_bounded_request(
                                context_builder,
                                &state.workspace_root,
                                &goal_for_llm,
                                session.as_deref_mut(),
                                &session_messages_snapshot,
                                &state.history,
                            );

                        write_event(
                            &state.run_id,
                            "context_stats",
                            &bounded_stats.to_json_string(),
                            0,
                        );
                        if reduction.trimmed {
                            let detail = json!({
                                "budget_tokens": reduction.budget_tokens,
                                "dropped_session_messages": reduction.dropped_session_messages,
                                "full_estimated_tokens": full_stats.estimated_tokens,
                                "bounded_estimated_tokens": bounded_stats.estimated_tokens
                            })
                            .to_string();
                            write_event(&state.run_id, "context_reduction", &detail, 0);
                        }

                        match llm.next_actions(&request).await {
                            Ok(calls) if !calls.is_empty() => {
                                println!(
                                    "[agent] {} tool call(s) after tool_use_failed retry {}",
                                    calls.len(),
                                    attempt
                                );
                                break 'next_actions calls;
                            }
                            Ok(_) => {
                                msg = "model returned empty tool_calls".to_string();
                                if attempt == tool_use_max_retries {
                                    write_event(
                                        &state.run_id,
                                        "tool_use_failed",
                                        &msg,
                                        0,
                                    );
                                    let reason = StopReason::ToolError;
                                    write_summary(state, &reason);
                                    return reason;
                                }
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
                                    write_summary(state, &reason);
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

        let model_turn = state.model_turns;
        state.model_turns += 1;

        // ── 3. Execute tool calls — consecutive read-only groups in parallel ─
        let mut verified_in_batch = false;
        let mut call_idx = 0;
        while call_idx < tool_calls.len() {
            if is_parallel_readonly(&tool_calls[call_idx].action) {
                let mut end = call_idx + 1;
                while end < tool_calls.len()
                    && is_parallel_readonly(&tool_calls[end].action)
                {
                    end += 1;
                }
                let group = &tool_calls[call_idx..end];
                if let Some(reason) = execute_readonly_group(
                    state,
                    group,
                    model_turn,
                    &mut verified_in_batch,
                    observer,
                )
                .await
                {
                    return reason;
                }
                call_idx = end;
            } else {
                if let Some(reason) = execute_one_tool(
                    state,
                    &tool_calls[call_idx],
                    model_turn,
                    require_verified_finish,
                    &mut verified_in_batch,
                    observer,
                )
                .await
                {
                    return reason;
                }
                call_idx += 1;
            }
        }
    }
}

/// Read-only tools that may run concurrently within a model turn.
fn is_parallel_readonly(action: &Action) -> bool {
    matches!(
        action,
        Action::ReadFile { .. }
            | Action::ListFiles { .. }
            | Action::SearchCode { .. }
            | Action::FindFiles { .. }
            | Action::DetectProjectSnapshot { .. }
            | Action::GitStatus
            | Action::GitDiff { .. }
    )
}

/// Execute one tool (mutating or any single tool) and commit it to history.
async fn execute_one_tool(
    state: &mut AgentState,
    tool_call: &LlmToolCall,
    model_turn: u32,
    require_verified_finish: bool,
    verified_in_batch: &mut bool,
    observer: Option<&dyn AgentObserver>,
) -> Option<StopReason> {
    let action = tool_call.action.clone();
    let tool_call_id = tool_call.tool_call_id.clone();
    let step_index = state.step_count;

    emit(
        observer,
        AgentEvent::StepStarted {
            run_id: state.run_id.clone(),
            step_index,
        },
    );
    emit(
        observer,
        AgentEvent::ToolStarted {
            run_id: state.run_id.clone(),
            step_index,
            tool: tool_name(&action).to_string(),
        },
    );

    let verified_for_finish = finish_allowed(&state.history) || *verified_in_batch;
    let mut result = produce_tool_result(
        state,
        &action,
        require_verified_finish,
        verified_for_finish,
    );

    if result.success {
        if let Some(path) = mutating_path(&action) {
            if let Some(err) = post_edit_sanity_check(&state.workspace_root, &path) {
                write_event(&state.run_id, "post_edit_sanity_failed", &err, 0);
                result.success = false;
                result.error = Some(err);
            }
        }
    }

    if result.success
        && matches!(
            action,
            Action::RunCommand { .. } | Action::RunShell { .. }
        )
    {
        *verified_in_batch = true;
    }

    commit_tool_outcome(
        state,
        observer,
        step_index,
        model_turn,
        action,
        tool_call_id,
        result,
    )
}

/// Run a consecutive group of read-only tools concurrently (order preserved in history).
async fn execute_readonly_group(
    state: &mut AgentState,
    group: &[LlmToolCall],
    model_turn: u32,
    verified_in_batch: &mut bool,
    observer: Option<&dyn AgentObserver>,
) -> Option<StopReason> {
    if group.len() == 1 {
        return execute_one_tool(
            state,
            &group[0],
            model_turn,
            false,
            verified_in_batch,
            observer,
        )
        .await;
    }

    state.parallel_tool_groups += 1;
    println!(
        "[agent] parallel read-only group ({} tools): {}",
        group.len(),
        group
            .iter()
            .map(|c| tool_name(&c.action))
            .collect::<Vec<_>>()
            .join(", ")
    );
    write_event(
        &state.run_id,
        "parallel_tool_group",
        &format!("{} tools", group.len()),
        0,
    );

    let base_step = state.step_count;
    let workspace = state.workspace_root.clone();

    // Emit start events + validate; collect spawn jobs for valid tools.
    let mut prepared: Vec<(u32, Action, String, Result<(), ValidationError>)> =
        Vec::with_capacity(group.len());

    for (offset, tool_call) in group.iter().enumerate() {
        let step_index = base_step + offset as u32;
        let action = tool_call.action.clone();
        let tool_call_id = tool_call.tool_call_id.clone();

        emit(
            observer,
            AgentEvent::StepStarted {
                run_id: state.run_id.clone(),
                step_index,
            },
        );
        emit(
            observer,
            AgentEvent::ToolStarted {
                run_id: state.run_id.clone(),
                step_index,
                tool: tool_name(&action).to_string(),
            },
        );

        let validation = validate_action(&action, false, false);
        if let Some(err) = &validation {
            write_event(&state.run_id, &err.event, &err.message, 0);
        }
        prepared.push((
            step_index,
            action,
            tool_call_id,
            match validation {
                Some(e) => Err(e),
                None => Ok(()),
            },
        ));
    }

    // Spawn blocking work for tools that passed validation.
    // Read-file hits the in-run cache on this thread before spawn.
    let mut handles = Vec::new();
    for (i, (step_index, action, tool_call_id, validation)) in prepared.into_iter().enumerate() {
        match validation {
            Err(e) => {
                handles.push((
                    i,
                    step_index,
                    action,
                    tool_call_id,
                    Some(ToolResult {
                        success: false,
                        output: String::new(),
                        diff: None,
                        error: Some(e.message),
                        stop_reason: None,
                    }),
                    None,
                    false,
                ));
            }
            Ok(()) => {
                if let Action::ReadFile {
                    path,
                    offset,
                    limit,
                } = &action
                {
                    if let Some(hit) =
                        state
                            .read_cache
                            .try_hit(&workspace, path, *offset, *limit)
                    {
                        write_event(
                            &state.run_id,
                            "cache_hit",
                            &format!("{} offset={} limit={}", path.display(), offset, limit),
                            0,
                        );
                        handles.push((
                            i,
                            step_index,
                            action,
                            tool_call_id,
                            Some(hit),
                            None,
                            true,
                        ));
                        continue;
                    }
                    write_event(
                        &state.run_id,
                        "cache_miss",
                        &format!("{} offset={} limit={}", path.display(), offset, limit),
                        0,
                    );
                }

                let root = workspace.clone();
                let action_for_job = action.clone();
                let handle =
                    tokio::task::spawn_blocking(move || tools::dispatch(&action_for_job, &root));
                handles.push((i, step_index, action, tool_call_id, None, Some(handle), false));
            }
        }
    }

    // Await in original order.
    let mut outcomes = Vec::with_capacity(handles.len());
    for (_i, step_index, action, tool_call_id, early, handle, from_cache) in handles {
        let result = if let Some(r) = early {
            r
        } else if let Some(h) = handle {
            match h.await {
                Ok(r) => r,
                Err(e) => ToolResult::error(format!("parallel tool join failed: {e}")),
            }
        } else {
            ToolResult::error("internal error: missing parallel tool result")
        };
        outcomes.push((step_index, action, tool_call_id, result, from_cache));
    }

    for (step_index, action, tool_call_id, result, from_cache) in outcomes {
        if result.success
            && matches!(action, Action::DetectProjectSnapshot { .. })
        {
            state.project_snapshot =
                Some(tools::project_snapshot::detect(&state.workspace_root));
        }

        if result.success && !from_cache {
            if let Action::ReadFile {
                path,
                offset,
                limit,
            } = &action
            {
                state.read_cache.store(
                    &state.workspace_root,
                    path,
                    *offset,
                    *limit,
                    step_index,
                    &result.output,
                );
            }
        }

        if let Some(reason) = commit_tool_outcome(
            state,
            observer,
            step_index,
            model_turn,
            action,
            tool_call_id,
            result,
        ) {
            return Some(reason);
        }
    }

    None
}

fn produce_tool_result(
    state: &mut AgentState,
    action: &Action,
    require_verified_finish: bool,
    verified_for_finish: bool,
) -> ToolResult {
    match validate_action(action, require_verified_finish, verified_for_finish) {
        Some(ValidationError { event, message }) => {
            write_event(&state.run_id, &event, &message, 0);
            ToolResult {
                success: false,
                output: String::new(),
                diff: None,
                error: Some(message),
                stop_reason: None,
            }
        }
        None => match action {
            Action::UpdatePlan { objective, steps } => {
                match crate::state::AgentPlan::from_update(objective, steps) {
                    Ok(plan) => {
                        let output = format!("Plan updated:\n{}", plan.render());
                        state.plan = Some(plan);
                        ToolResult {
                            success: true,
                            output,
                            diff: None,
                            error: None,
                            stop_reason: None,
                        }
                    }
                    Err(message) => {
                        write_event(&state.run_id, "invalid_tool_args", &message, 0);
                        ToolResult {
                            success: false,
                            output: String::new(),
                            diff: None,
                            error: Some(message),
                            stop_reason: None,
                        }
                    }
                }
            }
            Action::DetectProjectSnapshot { path } => {
                let result = tools::project_snapshot::execute(path, &state.workspace_root);
                if result.success {
                    state.project_snapshot =
                        Some(tools::project_snapshot::detect(&state.workspace_root));
                }
                result
            }
            Action::Finish { summary } => {
                let stop_reason = if verified_for_finish {
                    StopReason::TaskComplete
                } else {
                    StopReason::CompletedUnverified
                };
                ToolResult {
                    success: true,
                    output: summary.clone(),
                    diff: None,
                    error: None,
                    stop_reason: Some(stop_reason),
                }
            }
            Action::ReadFile {
                path,
                offset,
                limit,
            } => read_file_cached(state, path, *offset, *limit),
            Action::RunShell {
                command,
                cwd,
                timeout_secs,
            } => run_shell_deduped(state, command, cwd, *timeout_secs),
            Action::RunCommand {
                cmd,
                cwd,
                timeout_secs,
            } => run_command_deduped(state, cmd, cwd, *timeout_secs),
            _ => tools::dispatch(action, &state.workspace_root),
        },
    }
}

fn run_shell_deduped(
    state: &mut AgentState,
    command: &str,
    cwd: &std::path::Path,
    timeout_secs: u64,
) -> ToolResult {
    if let Some(cached) = state
        .shell_cache
        .before_execute("run_shell", command, cwd)
    {
        let event = if cached.success {
            "shell_cache_hit"
        } else {
            "shell_repeat_blocked"
        };
        write_event(
            &state.run_id,
            event,
            &format!("run_shell cwd={} cmd={}", cwd.display(), command),
            0,
        );
        return cached;
    }

    let result = tools::run_shell::execute(
        tools::run_shell::RunShellOpts {
            command,
            cwd,
            timeout_secs,
        },
        &state.workspace_root,
    );
    state.shell_cache.store_live(
        "run_shell",
        command,
        cwd,
        state.step_count,
        &result,
    );
    result
}

fn run_command_deduped(
    state: &mut AgentState,
    cmd: &str,
    cwd: &std::path::Path,
    timeout_secs: u64,
) -> ToolResult {
    if let Some(cached) = state
        .shell_cache
        .before_execute("run_command", cmd, cwd)
    {
        let event = if cached.success {
            "shell_cache_hit"
        } else {
            "shell_repeat_blocked"
        };
        write_event(
            &state.run_id,
            event,
            &format!("run_command cwd={} cmd={}", cwd.display(), cmd),
            0,
        );
        return cached;
    }

    let result = tools::run_command::execute(
        tools::run_command::RunCommandOpts {
            cmd,
            cwd,
            timeout_secs,
        },
        &state.workspace_root,
    );
    state.shell_cache.store_live(
        "run_command",
        cmd,
        cwd,
        state.step_count,
        &result,
    );
    result
}

fn read_file_cached(
    state: &mut AgentState,
    path: &std::path::Path,
    offset: u32,
    limit: u32,
) -> ToolResult {
    let step_index = state.step_count;
    if let Some(hit) = state
        .read_cache
        .try_hit(&state.workspace_root, path, offset, limit)
    {
        write_event(
            &state.run_id,
            "cache_hit",
            &format!("{} offset={} limit={}", path.display(), offset, limit),
            0,
        );
        return hit;
    }

    write_event(
        &state.run_id,
        "cache_miss",
        &format!("{} offset={} limit={}", path.display(), offset, limit),
        0,
    );

    let result = tools::read_file::execute(path, offset, limit, &state.workspace_root);
    if result.success {
        state.read_cache.store(
            &state.workspace_root,
            path,
            offset,
            limit,
            step_index,
            &result.output,
        );
    }
    result
}

/// Log, observe, and record a completed tool. Returns Some(stop) if the run should end.
fn commit_tool_outcome(
    state: &mut AgentState,
    observer: Option<&dyn AgentObserver>,
    step_index: u32,
    model_turn: u32,
    action: Action,
    tool_call_id: String,
    result: ToolResult,
) -> Option<StopReason> {
    emit(
        observer,
        AgentEvent::ToolCompleted {
            run_id: state.run_id.clone(),
            step_index,
            tool: tool_name(&action).to_string(),
            success: result.success,
        },
    );

    if result.success {
        if let Some(path) = mutating_path(&action) {
            state.note_edit();
            state.read_cache.invalidate_path(std::path::Path::new(&path));
            write_event(
                &state.run_id,
                "cache_invalidate",
                &path,
                0,
            );
            emit(
                observer,
                AgentEvent::FileModified {
                    run_id: state.run_id.clone(),
                    step_index,
                    path,
                },
            );
        }
        if matches!(
            action,
            Action::RunCommand { .. } | Action::RunShell { .. }
        ) {
            state.note_verification();
        }
    } else {
        emit(
            observer,
            AgentEvent::Error {
                run_id: state.run_id.clone(),
                step_index: Some(step_index),
                message: result
                    .error
                    .clone()
                    .unwrap_or_else(|| "tool failed".to_string()),
            },
        );
    }

    if let Action::RunCommand { cmd, .. } | Action::RunShell { command: cmd, .. } = &action {
        emit(
            observer,
            AgentEvent::CommandCompleted {
                run_id: state.run_id.clone(),
                step_index,
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

    if let Some(diff) = &result.diff {
        if !diff.is_empty() {
            let preview: String = diff.lines().take(10).collect::<Vec<_>>().join("\n");
            println!("[agent] diff preview:\n{}", preview);
            if diff.lines().count() > 10 {
                println!("[agent] ... ({} total diff lines)", diff.lines().count());
            }
        }
    }

    let stop_reason = result.stop_reason.clone();
    debug_assert_eq!(
        step_index, state.step_count,
        "step_index must match sequential record order"
    );
    let step = Step::new(state.step_count, model_turn, action, result, tool_call_id);
    let entry = LogEntry::from_step(&state.run_id, &state.model_id, &step);
    write_entry(&entry);
    state.record_step(step);

    emit(
        observer,
        AgentEvent::StepCompleted {
            run_id: state.run_id.clone(),
            step_index: state.step_count - 1,
            stop_reason: stop_reason.as_ref().map(|r| format!("{:?}", r)),
        },
    );

    if let Some(reason) = stop_reason {
        println!("\n[agent] stopping: {:?}", reason);
        write_summary(state, &reason);
        Some(reason)
    } else {
        None
    }
}

fn write_summary(state: &AgentState, reason: &StopReason) {
    write_run_summary(
        &state.run_id,
        &state.model_id,
        reason,
        &state.efficiency_snapshot(),
    );
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

fn goal_with_plan(state: &AgentState) -> String {
    let mut out = state.goal.clone();
    if let Some(snap) = &state.project_snapshot {
        out.push_str("\n\n## Project snapshot\n");
        out.push_str(&snap.render_compact());
    }
    if let Some(plan) = &state.plan {
        out.push_str("\n\n## Current plan\n");
        out.push_str(&plan.render());
    }
    out
}

fn looks_like_unified_diff(diff: &str) -> bool {
    let d = diff.trim_start();
    (d.starts_with("--- ") || d.contains("\n--- "))
        && (d.starts_with("+++ ") || d.contains("\n+++ "))
        && (d.starts_with("@@") || d.contains("\n@@"))
}

fn finish_allowed(history: &[Step]) -> bool {
    match history.last() {
        Some(step) => {
            matches!(
                step.action,
                crate::state::step::Action::RunCommand { .. }
                    | crate::state::step::Action::RunShell { .. }
            ) && step.result.success
        }
        None => false,
    }
}

struct ValidationError {
    event: String,
    message: String,
}

fn validate_action(
    action: &crate::state::step::Action,
    require_verified_finish: bool,
    verified_for_finish: bool,
) -> Option<ValidationError> {
    use crate::state::step::Action;

    match action {
        Action::ReadFile { path, .. } => validate_rel_path("read_file", path),
        Action::ListFiles { path } => validate_rel_path("list_files", path),
        Action::SearchCode { path, .. } => validate_rel_path("search_code", path),
        Action::FindFiles { path, .. } => validate_rel_path("find_files", path),
        Action::DetectProjectSnapshot { path } => {
            validate_rel_path("detect_project_snapshot", path)
        }
        Action::GitStatus => None,
        Action::GitDiff { path } => {
            if let Some(p) = path {
                validate_rel_path("git_diff", p)
            } else {
                None
            }
        }
        Action::WriteFile { path, .. } => validate_rel_path("write_file", path),
        Action::ApplyPatch { path, diff } => {
            if let Some(e) = validate_rel_path("apply_patch", path) {
                return Some(e);
            }
            if !looks_like_unified_diff(diff) {
                return Some(ValidationError {
                    event: "invalid_apply_patch_diff".to_string(),
                    message:
                        "apply_patch.diff must be a unified diff (must contain '---', '+++', and '@@')."
                            .to_string(),
                });
            }
            None
        }
        Action::RunCommand { cmd, cwd, .. } => {
            if let Some(e) = validate_run_command(cmd) {
                return Some(e);
            }
            validate_rel_path("run_command.working_directory", cwd)
        }
        Action::RunShell { command, cwd, .. } => {
            if command.trim().is_empty() {
                return Some(ValidationError {
                    event: "invalid_tool_args".to_string(),
                    message: "run_shell command must not be empty".to_string(),
                });
            }
            validate_rel_path("run_shell.working_directory", cwd)
        }
        Action::UpdatePlan { objective, steps } => {
            if objective.trim().is_empty() {
                return Some(ValidationError {
                    event: "invalid_tool_args".to_string(),
                    message: "update_plan objective must not be empty".to_string(),
                });
            }
            if steps.trim().is_empty() {
                return Some(ValidationError {
                    event: "invalid_tool_args".to_string(),
                    message: "update_plan steps must not be empty".to_string(),
                });
            }
            None
        }
        Action::Finish { .. } => {
            if require_verified_finish && !verified_for_finish {
                Some(ValidationError {
                    event: "finish_blocked".to_string(),
                    message: "finish blocked: run tests successfully before calling finish"
                        .to_string(),
                })
            } else {
                None
            }
        }
    }
}

fn validate_rel_path(tool: &str, path: &std::path::PathBuf) -> Option<ValidationError> {
    use std::path::Component;

    if path.is_absolute() {
        return Some(ValidationError {
            event: "invalid_tool_args".to_string(),
            message: format!("{tool} path must be relative (not absolute): {}", path.display()),
        });
    }

    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Some(ValidationError {
            event: "invalid_tool_args".to_string(),
            message: format!(
                "{tool} path must not contain '..' path traversal: {}",
                path.display()
            ),
        });
    }

    None
}

fn validate_run_command(cmd: &str) -> Option<ValidationError> {
    let trimmed = cmd.trim();
    if trimmed.is_empty() {
        return Some(ValidationError {
            event: "invalid_tool_args".to_string(),
            message: "run_command cmd must be non-empty".to_string(),
        });
    }

    let banned = ["&&", "||", "|", ">", "<"];
    if banned.iter().any(|b| trimmed.contains(b)) {
        return Some(ValidationError {
            event: "invalid_tool_args".to_string(),
            message: "run_command cmd must not contain shell operators (&&, ||, |, >, <)".to_string(),
        });
    }

    None
}

fn post_edit_sanity_check(workspace_root: &std::path::Path, rel_path: &str) -> Option<String> {
    use std::path::Path;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let rel = rel_path.trim();
    if rel.is_empty() {
        return None;
    }

    let ext = Path::new(rel)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    // Only cheap, non-invasive checks.
    let (program, args): (&str, Vec<String>) = match ext.as_str() {
        "js" | "cjs" | "mjs" => {
            // Syntax-only parse, no execution: new Function(source)
            let script = format!(
                "const fs=require('fs'); const src=fs.readFileSync({:?},'utf8'); new Function(src); console.log('ok');",
                rel
            );
            ("node", vec!["-e".to_string(), script])
        }
        "py" => ("python3", vec!["-m".to_string(), "py_compile".to_string(), rel.to_string()]),
        _ => return None,
    };

    let mut child = match Command::new(program)
        .args(args)
        .current_dir(workspace_root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", std::env::var("HOME").unwrap_or_default())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return Some(format!("sanity check failed to spawn {}: {}", program, e)),
    };

    let timeout = Duration::from_secs(5);
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    return Some(format!(
                        "sanity check timed out ({}s): {} {}",
                        timeout.as_secs(),
                        program,
                        rel
                    ));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Some(format!("sanity check wait error: {}", e)),
        }
    }

    let out = match child.wait_with_output() {
        Ok(o) => o,
        Err(e) => return Some(format!("sanity check output error: {}", e)),
    };

    if out.status.success() {
        None
    } else {
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        let msg = if stderr.is_empty() { stdout } else { stderr };
        Some(format!(
            "post-edit sanity check failed for {}: {}",
            rel,
            msg.trim()
        ))
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
    write_summary(state, &reason);
    reason
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn classifies_readonly_vs_mutating() {
        assert!(is_parallel_readonly(&Action::GitStatus));
        assert!(is_parallel_readonly(&Action::ReadFile {
            path: PathBuf::from("a.rs"),
            offset: 1,
            limit: 10,
        }));
        assert!(is_parallel_readonly(&Action::SearchCode {
            query: "x".into(),
            path: PathBuf::from("."),
        }));
        assert!(is_parallel_readonly(&Action::FindFiles {
            pattern: "*.rs".into(),
            path: PathBuf::from("."),
        }));
        assert!(is_parallel_readonly(&Action::ListFiles {
            path: PathBuf::from("."),
        }));
        assert!(is_parallel_readonly(&Action::GitDiff { path: None }));
        assert!(is_parallel_readonly(&Action::DetectProjectSnapshot {
            path: PathBuf::from("."),
        }));

        assert!(!is_parallel_readonly(&Action::WriteFile {
            path: PathBuf::from("a"),
            content: String::new(),
        }));
        assert!(!is_parallel_readonly(&Action::ApplyPatch {
            path: PathBuf::from("a"),
            diff: String::new(),
        }));
        assert!(!is_parallel_readonly(&Action::RunCommand {
            cmd: "ls".into(),
            cwd: PathBuf::from("."),
            timeout_secs: 30,
        }));
        assert!(!is_parallel_readonly(&Action::RunShell {
            command: "ls".into(),
            cwd: PathBuf::from("."),
            timeout_secs: 30,
        }));
        assert!(!is_parallel_readonly(&Action::UpdatePlan {
            objective: "o".into(),
            steps: "- [ ] a".into(),
        }));
        assert!(!is_parallel_readonly(&Action::Finish {
            summary: "done".into(),
        }));
    }
}
