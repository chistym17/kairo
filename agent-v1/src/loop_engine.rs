use crate::authorization::{
    apply_decision, authorize, build_approval_request, tool_effect, ApprovalDecision,
    ApprovalGate, AuthorizationDecision,
};
use crate::context::{ContextBuilder, ContextManager};
use crate::governance::{classify_progress, GovernanceDecision};
use crate::llm::{render_call_line, LlmClient, LlmError, LlmToolCall, ModelUsage};
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
    approval_gate: Option<&dyn ApprovalGate>,
) -> StopReason {
    if let Some(session) = session.as_deref_mut() {
        state.authorization = session.authorization.clone();
        state.task = session.task.clone();
        if session.intent_pinned {
            state.turn_intent = session.declared_intent;
        } else {
            state.turn_intent = crate::session::TurnIntent::infer_from_message(&state.goal);
            session.declared_intent = state.turn_intent;
        }
    } else {
        state.turn_intent = crate::session::TurnIntent::infer_from_message(&state.goal);
    }
    state.task.set_objective_if_empty(&state.goal);

    write_event(
        &state.run_id,
        "turn_intent",
        &format!(
            "{}{}",
            state.turn_intent.as_str(),
            if session
                .as_ref()
                .map(|s| s.intent_pinned)
                .unwrap_or(false)
            {
                " (pinned)"
            } else {
                " (inferred)"
            }
        ),
        0,
    );

    println!(
        "\n[agent] starting run {}\n[agent] goal: {}\n[agent] model: {}\n[agent] max_steps: {}\n[agent] edit_policy: {:?}\n[agent] turn_intent: {} (metadata)\n",
        state.run_id,
        state.goal,
        state.model_id,
        state.max_steps,
        state.authorization.edit_policy,
        state.turn_intent.as_str()
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

    // Hybrid locate index once per run (incremental updates on edit).
    if state.repo_intel.is_none() {
        match crate::repo_intel::HybridIntel::build(
            &state.workspace_root,
            state.workspace_root.display().to_string(),
        ) {
            Ok((intel, stats)) => {
                println!(
                    "[agent] repo index: {} file(s) in {}ms{}",
                    stats.files_indexed,
                    stats.build_ms,
                    if stats.truncated { " (truncated)" } else { "" }
                );
                write_event(
                    &state.run_id,
                    "repo_index_built",
                    &format!(
                        "files={} skipped={} build_ms={} truncated={}",
                        stats.files_indexed,
                        stats.files_skipped,
                        stats.build_ms,
                        stats.truncated
                    ),
                    stats.build_ms as u64,
                );
                state.repo_intel = Some(intel);
            }
            Err(e) => {
                println!("[agent] repo index build failed: {e} — locate will rebuild per call");
                write_event(&state.run_id, "repo_index_build_failed", &e.to_string(), 0);
            }
        }
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
            let tool_policy = if state.governance.semantic_policy {
                Some(state.governance.tool_policy())
            } else {
                None
            };
            let (request, full_stats, bounded_stats, reduction) =
                context_manager.build_bounded_request(
                    context_builder,
                    &state.workspace_root,
                    &goal_for_llm,
                    session.as_deref_mut(),
                    &session_messages_snapshot,
                    &state.history,
                    tool_policy.as_ref(),
                );

            write_event(&state.run_id, "context_stats", &bounded_stats.to_json_string(), 0);
            log_prompt_diagnostics(state, &request.diagnostics);
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

            let (actions_result, usage) = llm.next_actions_with_usage(&request).await;
            if let Some(usage) = usage {
                record_model_usage(state, usage);
            }
            match actions_result {
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
                    let actions: Vec<_> = calls.iter().map(|c| c.action.clone()).collect();
                    state.exploration.observe_turn(&actions);
                    state.governance.note_tool_turn();
                    let level = state.exploration.no_edit_escalation_level();
                    state.governance.set_no_edit_escalation_level(level);
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
                    state.exploration.model_turns_since_edit =
                        state.exploration.model_turns_since_edit.saturating_add(1);
                    state.exploration.consecutive_single_readonly_turns = 0;
                    state.governance.note_no_tool_turn();
                    if state.governance.semantic_policy
                        && state.governance.consecutive_no_tool_turns
                            >= crate::governance::MAX_NO_TOOL_TURNS
                    {
                        state.governance.pending_guidance = Some(
                            "You did not request a tool or finish the task. \
                             Next response must choose one: apply_patch/write_file, one justified \
                             read_file, update_plan, or finish with a blocker."
                                .to_string(),
                        );
                        if state.governance.consecutive_no_tool_turns
                            > crate::governance::MAX_NO_TOOL_TURNS
                        {
                            state.governance.enter_edit_or_justify(
                                "consecutive text-only turns with no action.",
                            );
                        }
                    }
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
                        let tool_policy = if state.governance.semantic_policy {
                            Some(state.governance.tool_policy())
                        } else {
                            None
                        };
                        let (request, full_stats, bounded_stats, reduction) =
                            context_manager.build_bounded_request(
                                context_builder,
                                &state.workspace_root,
                                &goal_for_llm,
                                session.as_deref_mut(),
                                &session_messages_snapshot,
                                &state.history,
                                tool_policy.as_ref(),
                            );

                        write_event(
                            &state.run_id,
                            "context_stats",
                            &bounded_stats.to_json_string(),
                            0,
                        );
                        log_prompt_diagnostics(state, &request.diagnostics);
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

                        let (actions_result, usage) = llm.next_actions_with_usage(&request).await;
                        if let Some(usage) = usage {
                            record_model_usage(state, usage);
                        }
                        match actions_result {
                            Ok(calls) if !calls.is_empty() => {
                                println!(
                                    "[agent] {} tool call(s) after tool_use_failed retry {}",
                                    calls.len(),
                                    attempt
                                );
                                let actions: Vec<_> =
                                    calls.iter().map(|c| c.action.clone()).collect();
                                state.exploration.observe_turn(&actions);
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
                    approval_gate,
                    &mut session,
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
                    approval_gate,
                    &mut session,
                )
                .await
                {
                    return reason;
                }
                call_idx += 1;
            }
        }

        state.governance.end_turn(
            model_turn,
            state
                .history
                .iter()
                .rev()
                .take_while(|s| s.model_turn == model_turn)
                .any(|s| {
                    matches!(
                        s.action,
                        Action::ApplyPatch { .. } | Action::WriteFile { .. }
                    ) && s.result.success
                }),
        );

        if state.completion.should_complete_unverified(model_turn) {
            let reason = StopReason::CompletedUnverified;
            write_event(
                &state.run_id,
                "post_edit_idle_complete",
                &format!(
                    "edit+review+verify attempted; idle >= {} turns — CompletedUnverified",
                    crate::state::completion::POST_EDIT_IDLE_TURNS
                ),
                0,
            );
            println!(
                "\n[agent] post-edit idle complete — {:?}",
                reason
            );
            write_summary(state, &reason);
            return reason;
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
            | Action::Locate { .. }
            | Action::FindFiles { .. }
            | Action::DetectProjectSnapshot { .. }
            | Action::SummarizeRepo { .. }
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
    approval_gate: Option<&dyn ApprovalGate>,
    session: &mut Option<&mut Session>,
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

    match apply_governance_preflight(state, &action, model_turn) {
        GovernancePreflightOutcome::Proceed => {}
        GovernancePreflightOutcome::Rejected(result) => {
            return commit_tool_outcome(
                state,
                observer,
                step_index,
                model_turn,
                action,
                tool_call_id,
                result,
            );
        }
        GovernancePreflightOutcome::Terminate(reason) => {
            write_summary(state, &reason);
            return Some(reason);
        }
    }

    if let Err(denied) = apply_evidence_checkpoint_gate(state, &action) {
        return commit_tool_outcome(
            state,
            observer,
            step_index,
            model_turn,
            action,
            tool_call_id,
            denied,
        );
    }

    if let Err(denied) = apply_shell_mutation_gate(state, &action) {
        return commit_tool_outcome(
            state,
            observer,
            step_index,
            model_turn,
            action,
            tool_call_id,
            denied,
        );
    }

    if let Err(denied) = resolve_tool_authorization(state, &action, approval_gate, session) {
        return commit_tool_outcome(
            state,
            observer,
            step_index,
            model_turn,
            action,
            tool_call_id,
            denied,
        );
    }

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

    let plan_update_version =
        if result.success && matches!(action, Action::UpdatePlan { .. }) {
            Some(
                state
                    .task
                    .plan
                    .as_ref()
                    .map(|p| p.version)
                    .unwrap_or(0),
            )
        } else {
            None
        };

    let stop = commit_tool_outcome(
        state,
        observer,
        step_index,
        model_turn,
        action,
        tool_call_id,
        result,
    );
    // Persist task memory (plan / files / verification / decisions) when a session is attached.
    sync_task_to_session(state, session);
    if let Some(version) = plan_update_version {
        if let Some(session) = session.as_deref_mut() {
            session.append_session_update(format!("Plan updated to v{version}."));
        }
    }
    stop
}

/// Run a consecutive group of read-only tools concurrently (order preserved in history).
async fn execute_readonly_group(
    state: &mut AgentState,
    group: &[LlmToolCall],
    model_turn: u32,
    verified_in_batch: &mut bool,
    observer: Option<&dyn AgentObserver>,
    approval_gate: Option<&dyn ApprovalGate>,
    session: &mut Option<&mut Session>,
) -> Option<StopReason> {
    if group.len() == 1 {
        return execute_one_tool(
            state,
            &group[0],
            model_turn,
            false,
            verified_in_batch,
            observer,
            approval_gate,
            session,
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
    enum PreparedOutcome {
        Early(ToolResult),
        Invalid(ValidationError),
        Execute,
    }

    let mut prepared: Vec<(u32, Action, String, PreparedOutcome)> =
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

        match apply_governance_preflight(state, &action, model_turn) {
            GovernancePreflightOutcome::Proceed => {}
            GovernancePreflightOutcome::Rejected(result) => {
                prepared.push((step_index, action, tool_call_id, PreparedOutcome::Early(result)));
                continue;
            }
            GovernancePreflightOutcome::Terminate(reason) => {
                write_summary(state, &reason);
                return Some(reason);
            }
        }

        if let Err(denied) = apply_evidence_checkpoint_gate(state, &action) {
            prepared.push((step_index, action, tool_call_id, PreparedOutcome::Early(denied)));
            continue;
        }

        if let Err(denied) = apply_shell_mutation_gate(state, &action) {
            prepared.push((step_index, action, tool_call_id, PreparedOutcome::Early(denied)));
            continue;
        }

        let validation = validate_action(&action, false, false);
        if let Some(err) = &validation {
            write_event(&state.run_id, &err.event, &err.message, 0);
        }
        prepared.push((
            step_index,
            action,
            tool_call_id,
            match validation {
                Some(e) => PreparedOutcome::Invalid(e),
                None => PreparedOutcome::Execute,
            },
        ));
    }

    // Spawn blocking work for tools that passed validation.
    // Read-file hits the in-run cache on this thread before spawn.
    let mut handles = Vec::new();
    for (i, (step_index, action, tool_call_id, outcome)) in prepared.into_iter().enumerate() {
        match outcome {
            PreparedOutcome::Early(result) => {
                handles.push((i, step_index, action, tool_call_id, Some(result), None, false));
            }
            PreparedOutcome::Invalid(e) => {
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
            PreparedOutcome::Execute => {
                if let Action::ReadFile {
                    path,
                    offset,
                    limit,
                } = &action
                {
                    match state.read_cache.resolve(&workspace, path, *offset, *limit) {
                        tools::read_cache::ReadResolve::AlreadyRead(hit) => {
                            write_event(
                                &state.run_id,
                                "cache_hit",
                                &format!(
                                    "{} offset={} limit={} (duplicate read denied)",
                                    path.display(),
                                    offset,
                                    limit
                                ),
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
                        tools::read_cache::ReadResolve::Live {
                            offset: live_off,
                            limit: live_lim,
                            note,
                        } => {
                            write_event(
                                &state.run_id,
                                if note.is_some() {
                                    "cache_partial"
                                } else {
                                    "cache_miss"
                                },
                                &format!(
                                    "{} requested={}-{} live={}-{}",
                                    path.display(),
                                    offset,
                                    offset.saturating_add(limit.saturating_sub(1)),
                                    live_off,
                                    live_off.saturating_add(live_lim.saturating_sub(1))
                                ),
                                0,
                            );
                            let root = workspace.clone();
                            let path_owned = path.clone();
                            let handle = tokio::task::spawn_blocking(move || {
                                let mut result =
                                    tools::read_file::execute(&path_owned, live_off, live_lim, &root);
                                if let Some(note) = note {
                                    if result.success {
                                        result.output = format!("{note}{}", result.output);
                                    }
                                }
                                result
                            });
                            handles.push((
                                i,
                                step_index,
                                action,
                                tool_call_id,
                                None,
                                Some(handle),
                                false,
                            ));
                            continue;
                        }
                    }
                }

                if let Action::Locate { query, path } = &action {
                    let result = locate_with_state(state, query, path);
                    handles.push((
                        i,
                        step_index,
                        action,
                        tool_call_id,
                        Some(result),
                        None,
                        false,
                    ));
                    continue;
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
                let updated = if let Some(existing) = state.task.plan.as_mut() {
                    existing.amend_from_update(objective, steps).map(|_| ())
                } else {
                    crate::state::AgentPlan::from_update(objective, steps).map(|plan| {
                        state.task.plan = Some(plan);
                    })
                };
                match updated {
                    Ok(()) => {
                        let output = format!(
                            "Plan updated:\n{}",
                            state
                                .task
                                .plan
                                .as_ref()
                                .expect("plan present after update")
                                .render()
                        );
                        state.task.set_objective_if_empty(objective);
                        state.governance.pre_edit.observe_plan(steps);
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
            Action::Locate { query, path } => locate_with_state(state, query, path),
            Action::SummarizeRepo { depth, path, focus } => {
                summarize_repo_with_state(state, depth, path, focus.as_deref())
            }
            Action::Finish { summary } => {
                let grounded = crate::state::ground_finish_summary(state, summary);
                let stop_reason = if verified_for_finish {
                    StopReason::TaskComplete
                } else {
                    StopReason::CompletedUnverified
                };
                ToolResult {
                    success: true,
                    output: grounded,
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
            Action::GitDiff { path } => {
                let ctx = tools::git_diff::GitDiffContext {
                    files_ever_modified: state
                        .governance
                        .files_ever_modified
                        .iter()
                        .cloned()
                        .collect(),
                };
                tools::git_diff::execute(path, &state.workspace_root, Some(&ctx))
            }
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
    if let Some(intent) = tools::shell_read::detect_shell_file_read(command) {
        if let Some(blocked) =
            tools::shell_read::apply_read_governance_to_shell(&mut state.read_cache, &state.workspace_root, &intent)
        {
            write_event(
                &state.run_id,
                "shell_read_governance",
                &format!("run_shell cwd={} cmd={}", cwd.display(), command),
                0,
            );
            return blocked;
        }
    }

    if let Some(blocked) = state.shell_cache.before_explore_budget(command) {
        write_event(
            &state.run_id,
            "shell_explore_budget_blocked",
            &format!("run_shell cwd={} cmd={}", cwd.display(), command),
            0,
        );
        return blocked;
    }

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
    state.shell_cache.note_explore_executed(command);
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
    if let Some(intent) = tools::shell_read::detect_shell_file_read(cmd) {
        if let Some(blocked) =
            tools::shell_read::apply_read_governance_to_shell(&mut state.read_cache, &state.workspace_root, &intent)
        {
            write_event(
                &state.run_id,
                "shell_read_governance",
                &format!("run_command cwd={} cmd={}", cwd.display(), cmd),
                0,
            );
            return blocked;
        }
    }

    if let Some(blocked) = state.shell_cache.before_explore_budget(cmd) {
        write_event(
            &state.run_id,
            "shell_explore_budget_blocked",
            &format!("run_command cwd={} cmd={}", cwd.display(), cmd),
            0,
        );
        return blocked;
    }

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
    state.shell_cache.note_explore_executed(cmd);
    state.shell_cache.store_live(
        "run_command",
        cmd,
        cwd,
        state.step_count,
        &result,
    );
    result
}

fn locate_with_state(
    state: &mut AgentState,
    query: &str,
    path: &std::path::Path,
) -> ToolResult {
    ensure_repo_intel(state);
    let scope = if path.as_os_str().is_empty() {
        std::path::PathBuf::from(".")
    } else {
        path.to_path_buf()
    };
    if let Some(intel) = state.repo_intel.as_ref() {
        crate::repo_intel::execute_with_intel(intel, query, &scope)
    } else {
        crate::repo_intel::execute(query, path, &state.workspace_root)
    }
}

fn summarize_repo_with_state(
    state: &mut AgentState,
    depth: &str,
    path: &std::path::Path,
    focus: Option<&str>,
) -> ToolResult {
    ensure_repo_intel(state);
    let Some(intel) = state.repo_intel.as_ref() else {
        return ToolResult::error(
            "summarize_repo: repository index unavailable (index build failed)",
        );
    };

    let depth = crate::repo_intel::SummaryDepth::parse(depth).unwrap_or(
        crate::repo_intel::SummaryDepth::Overview,
    );
    let scope = if path.as_os_str().is_empty() || path == std::path::Path::new(".") {
        crate::repo_intel::SummaryScope::Repository
    } else {
        crate::repo_intel::SummaryScope::Path(path.to_path_buf())
    };
    let request = crate::repo_intel::SummaryRequest {
        scope,
        depth,
        budget: crate::repo_intel::SummaryBudget::for_depth(depth),
        focus: focus.map(|s| s.to_string()),
    };

    let summary = crate::repo_intel::summarize(
        &state.workspace_root,
        intel.index(),
        state.project_snapshot.as_ref(),
        &request,
    );

    write_event(
        &state.run_id,
        "repo_summary",
        &format!(
            "depth={:?} files={} symbols={} truncated={} chars={}",
            depth,
            summary.files_mentioned,
            summary.symbols_mentioned,
            summary.truncated,
            summary.text.len()
        ),
        0,
    );

    ToolResult {
        success: true,
        output: summary.text,
        diff: None,
        error: None,
        stop_reason: None,
    }
}

fn ensure_repo_intel(state: &mut AgentState) {
    if state.repo_intel.is_some() {
        return;
    }
    if let Ok((intel, stats)) = crate::repo_intel::HybridIntel::build(
        &state.workspace_root,
        state.workspace_root.display().to_string(),
    ) {
        write_event(
            &state.run_id,
            "repo_index_built",
            &format!(
                "files={} skipped={} build_ms={} truncated={} (lazy)",
                stats.files_indexed, stats.files_skipped, stats.build_ms, stats.truncated
            ),
            stats.build_ms as u64,
        );
        state.repo_intel = Some(intel);
    }
}

fn read_file_cached(
    state: &mut AgentState,
    path: &std::path::Path,
    offset: u32,
    limit: u32,
) -> ToolResult {
    let step_index = state.step_count;
    match state
        .read_cache
        .resolve(&state.workspace_root, path, offset, limit)
    {
        tools::read_cache::ReadResolve::AlreadyRead(hit) => {
            write_event(
                &state.run_id,
                "cache_hit",
                &format!(
                    "{} offset={} limit={} (duplicate read denied)",
                    path.display(),
                    offset,
                    limit
                ),
                0,
            );
            hit
        }
        tools::read_cache::ReadResolve::Live {
            offset: live_off,
            limit: live_lim,
            note,
        } => {
            write_event(
                &state.run_id,
                if note.is_some() {
                    "cache_partial"
                } else {
                    "cache_miss"
                },
                &format!(
                    "{} requested={}-{} live={}-{}",
                    path.display(),
                    offset,
                    offset.saturating_add(limit.saturating_sub(1)),
                    live_off,
                    live_off.saturating_add(live_lim.saturating_sub(1))
                ),
                0,
            );

            let mut result =
                tools::read_file::execute(path, live_off, live_lim, &state.workspace_root);
            if let Some(note) = note {
                if result.success {
                    result.output = format!("{note}{}", result.output);
                }
            }
            if result.success {
                state.read_cache.store(
                    &state.workspace_root,
                    path,
                    live_off,
                    live_lim,
                    step_index,
                    &result.output,
                );
            }
            result
        }
    }
}

fn is_duplicate_read_outcome(action: &Action, result: &ToolResult) -> bool {
    if !matches!(action, Action::ReadFile { .. }) {
        return false;
    }
    if result.output.starts_with("[cached")
        || result.output.to_ascii_lowercase().contains("[already read")
    {
        return true;
    }
    result
        .error
        .as_deref()
        .map(|e| {
            let l = e.to_ascii_lowercase();
            l.contains("duplicate read denied") || l.contains("exact content already available")
        })
        .unwrap_or(false)
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
            state.note_edit(model_turn);
            state.governance.note_file_modified(&path);
            state.task.note_modified_file(std::path::PathBuf::from(&path));
            state.read_cache.invalidate_path(std::path::Path::new(&path));
            if let Some(intel) = state.repo_intel.as_mut() {
                let change = crate::repo_intel::FileChange {
                    path: std::path::PathBuf::from(&path),
                    kind: crate::repo_intel::FileChangeKind::Modified,
                };
                match intel.update_with_stats(&[change]) {
                    Ok(stats) => {
                        write_event(
                            &state.run_id,
                            "repo_index_updated",
                            &format!(
                                "path={} updated={} removed={} update_ms={}",
                                path, stats.files_updated, stats.files_removed, stats.update_ms
                            ),
                            stats.update_ms as u64,
                        );
                    }
                    Err(e) => {
                        write_event(
                            &state.run_id,
                            "repo_index_update_failed",
                            &format!("path={path} err={e}"),
                            0,
                        );
                    }
                }
            }
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

    // Objective completion milestones (not semantic governance).
    let duplicate_read = is_duplicate_read_outcome(&action, &result);
    let read_cache_hit = duplicate_read;
    let result_is_blocked = result
        .error
        .as_deref()
        .map(|e| {
            let l = e.to_ascii_lowercase();
            l.contains("explore-shell budget")
                || l.contains("blocked: identical")
                || l.contains("blocked: explore")
                || l.contains("duplicate read denied")
                || l.contains("exploration checkpoint active")
        })
        .unwrap_or(false);

    state
        .progress
        .observe_tool(&action, &result, duplicate_read);

    if !read_cache_hit
        && !result_is_blocked
        && !matches!(
            action,
            Action::ApplyPatch { .. } | Action::WriteFile { .. } | Action::Finish { .. }
        )
    {
        state
            .completion
            .note_novel_result(model_turn, objective_result_fingerprint(&action, &result));
    }

    match &action {
        Action::GitDiff { .. } | Action::GitStatus => {
            state.completion.note_review(model_turn);
        }
        Action::RunCommand { cmd, cwd, .. }
        | Action::RunShell {
            command: cmd,
            cwd,
            ..
        } => {
            // Only real executions matching repository-discovered checks count
            // as verification. Arbitrary shell commands no longer satisfy the
            // completion milestone.
            if !result_is_blocked {
                let matched = state.project_snapshot.as_ref().and_then(|snapshot| {
                    let checks = crate::verification::discover_checks(snapshot);
                    crate::verification::match_executed_check(cmd, cwd, &checks)
                        .map(|check| crate::verification::outcome_for(check, &result))
                });
                if let Some(outcome) = matched {
                    state.note_verification();
                    state.task.note_verification(&outcome);
                    state
                        .completion
                        .note_verification_outcome(model_turn, outcome);
                }
            }
        }
        _ => {}
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

    if result.success {
        match &action {
            Action::ReadFile { path, .. } => state.governance.pre_edit.observe_read(
                &path.to_string_lossy().replace('\\', "/"),
            ),
            Action::SearchCode { .. } => state.governance.pre_edit.observe_search(),
            Action::Locate { .. } => state.governance.pre_edit.observe_search(),
            _ => {}
        }
    }
    record_governance_after(state, &action, &result, model_turn, read_cache_hit);

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
    let totals = state.model_usage.totals();
    if let Ok(json) = serde_json::to_string(&totals) {
        write_event(&state.run_id, "usage_totals", &json, 0);
    }
}

fn record_model_usage(state: &mut AgentState, usage: ModelUsage) {
    state.model_usage.record(usage);
    if let Some(last) = state.model_usage.last() {
        println!("[usage] {}", render_call_line(last));
        if let Ok(json) = serde_json::to_string(last) {
            write_event(&state.run_id, "model_usage", &json, 0);
        }
    }
}

fn log_prompt_diagnostics(
    state: &mut AgentState,
    diagnostics: &crate::context::PromptDiagnostics,
) {
    println!("[prompt] {}", diagnostics.render_compact());
    if let Ok(json) = serde_json::to_string(diagnostics) {
        write_event(&state.run_id, "prompt_diagnostics", &json, 0);
    }
    if let Some(prev) = state.last_prompt_diagnostics.as_ref() {
        let cmp = crate::context::PrefixCompare::compare(prev, diagnostics);
        println!("[cache-verify] {}", cmp.render_compact());
        if let Ok(json) = serde_json::to_string(&cmp) {
            write_event(&state.run_id, "prefix_compare", &json, 0);
        }
    }
    state.last_prompt_diagnostics = Some(diagnostics.clone());
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
        Action::Locate { .. } => "locate",
        Action::FindFiles { .. } => "find_files",
        Action::DetectProjectSnapshot { .. } => "detect_project_snapshot",
        Action::SummarizeRepo { .. } => "summarize_repo",
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

fn goal_with_plan(state: &mut AgentState) -> String {
    let mut out = state.goal.clone();
    out.push_str("\n\n## Turn intent\n");
    out.push_str(state.turn_intent.prompt_hint());
    out.push('\n');
    if let Some(snap) = &state.project_snapshot {
        out.push_str("\n\n## Project snapshot\n");
        out.push_str(&snap.render_compact());
    }
    if let Some(plan) = &state.task.plan {
        out.push_str("\n\n## Current plan\n");
        out.push_str(&plan.render());
    }
    // Compact task memory (decisions / files / verification) when present.
    if state.task.objective.is_some()
        || !state.task.decisions.is_empty()
        || !state.task.modified_files.is_empty()
        || !state.task.verification.is_empty()
    {
        out.push_str("\n\n## Task state\n");
        out.push_str(&state.task.render_compact());
    }
    if state.completion.last_edit_turn.is_some() {
        let summary = crate::state::workspace_summary::render_workspace_summary(state);
        out.push_str("\n\n## Workspace state\n");
        out.push_str(&summary);
    }
    // Soft stagnation / evidence-checkpoint / after-decision guidance.
    let mut injected_guidance = false;
    if let Some(nudge) = state.progress.nudge_for_injection() {
        let event = if state.progress.checkpoint_active {
            "evidence_checkpoint"
        } else {
            "progress_stagnation_nudge"
        };
        write_event(
            &state.run_id,
            event,
            &nudge[..nudge.len().min(400)],
            0,
        );
        out.push_str("\n\n## Runtime guidance\n");
        out.push_str(&nudge);
        injected_guidance = true;
    }
    if !injected_guidance {
        if let Some(nudge) = state.progress.after_decision_guidance() {
            write_event(
                &state.run_id,
                "after_decision_guidance",
                &nudge[..nudge.len().min(400)],
                0,
            );
            out.push_str("\n\n## Runtime guidance\n");
            out.push_str(&nudge);
            injected_guidance = true;
        }
    }
    if !injected_guidance {
        if let Some(nudge) = state.exploration.nudge_for_injection() {
            write_event(
                &state.run_id,
                "exploration_batch_nudge",
                &nudge[..nudge.len().min(400)],
                0,
            );
            out.push_str("\n\n## Runtime guidance\n");
            out.push_str(&nudge);
        }
    }
    if state.governance.semantic_policy {
        let level = state.exploration.no_edit_escalation_level();
        state.governance.set_no_edit_escalation_level(level);
        if let Some(guidance) = state.governance.take_pending_guidance() {
            write_event(
                &state.run_id,
                "governance_guidance",
                &guidance[..guidance.len().min(400)],
                0,
            );
            out.push_str("\n\n## Runtime guidance\n");
            out.push_str(&guidance);
        }
    }
    out
}

fn apply_shell_mutation_gate(
    state: &mut AgentState,
    action: &Action,
) -> Result<(), ToolResult> {
    let command = match action {
        Action::RunShell { command, .. } => command.as_str(),
        Action::RunCommand { cmd, .. } => cmd.as_str(),
        _ => return Ok(()),
    };
    if !tools::shell_mutation::is_shell_workspace_mutation(command) {
        return Ok(());
    }
    let message = tools::shell_mutation::shell_mutation_blocked_message(command);
    write_event(
        &state.run_id,
        "shell_mutation_blocked",
        &message[..message.len().min(400)],
        0,
    );
    Err(ToolResult::error(message))
}

fn apply_evidence_checkpoint_gate(
    state: &mut AgentState,
    action: &Action,
) -> Result<(), ToolResult> {
    match state.progress.check_exploration_allowed(action) {
        Ok(()) => Ok(()),
        Err(message) => {
            write_event(
                &state.run_id,
                "evidence_checkpoint_denied",
                &message[..message.len().min(400)],
                0,
            );
            Err(ToolResult::error(message))
        }
    }
}

enum GovernancePreflightOutcome {
    Proceed,
    Rejected(ToolResult),
    Terminate(StopReason),
}

/// Classify → authorize → pause if needed → persist grant → allow or deny.
fn resolve_tool_authorization(
    state: &mut AgentState,
    action: &Action,
    approval_gate: Option<&dyn ApprovalGate>,
    session: &mut Option<&mut Session>,
) -> Result<(), ToolResult> {
    let effect = tool_effect(action);
    match authorize(effect, &state.authorization) {
        AuthorizationDecision::Allow => Ok(()),
        AuthorizationDecision::Deny(message) => {
            write_event(&state.run_id, "authorization_denied", &message, 0);
            Err(ToolResult::error(message))
        }
        AuthorizationDecision::RequireApproval(_) => {
            let request = build_approval_request(action, effect);
            write_event(
                &state.run_id,
                "authorization_required",
                &format!("{:?} — {}", effect, request.summary),
                0,
            );

            let Some(gate) = approval_gate else {
                let message = format!(
                    "authorization required for {} ({:?}) but no approval gate is available \
                     (use interactive session or EditPolicy::Auto)",
                    request.summary, effect
                );
                write_event(&state.run_id, "authorization_denied", &message, 0);
                return Err(ToolResult::error(message));
            };

            let decision = gate.request_approval(&request);
            write_event(
                &state.run_id,
                "authorization_decision",
                &format!("{decision:?} for {}", request.summary),
                0,
            );
            apply_decision(&mut state.authorization, effect, &decision);
            sync_authorization_to_session(state, session);
            append_authorization_session_update(session, effect, &decision);

            match decision {
                ApprovalDecision::Reject => {
                    let message = format!(
                        "user rejected authorization for {} ({:?})",
                        request.summary, effect
                    );
                    Err(ToolResult::error(message))
                }
                ApprovalDecision::ApproveOnce | ApprovalDecision::ApproveForTask => Ok(()),
            }
        }
    }
}

fn append_authorization_session_update(
    session: &mut Option<&mut Session>,
    effect: crate::authorization::ToolEffect,
    decision: &ApprovalDecision,
) {
    let Some(session) = session.as_deref_mut() else {
        return;
    };
    let body = match decision {
        ApprovalDecision::ApproveForTask => {
            format!(
                "Authorization: ApproveForTask for {:?} — workspace mutations approved for this task.",
                effect
            )
        }
        ApprovalDecision::ApproveOnce => {
            format!("Authorization: ApproveOnce for {:?}.", effect)
        }
        ApprovalDecision::Reject => {
            format!("Authorization: rejected for {:?}.", effect)
        }
    };
    session.append_session_update(body);
}

fn sync_authorization_to_session(state: &AgentState, session: &mut Option<&mut Session>) {
    if let Some(session) = session.as_deref_mut() {
        session.authorization = state.authorization.clone();
    }
}

fn sync_task_to_session(state: &AgentState, session: &mut Option<&mut Session>) {
    if let Some(session) = session.as_deref_mut() {
        session.task = state.task.clone();
    }
}

fn apply_governance_preflight(
    state: &mut AgentState,
    action: &Action,
    model_turn: u32,
) -> GovernancePreflightOutcome {
    match state.governance.before_action(action, model_turn) {
        GovernanceDecision::Allow => GovernancePreflightOutcome::Proceed,
        GovernanceDecision::Reject { message, .. } => {
            write_event(&state.run_id, "governance_rejected", &message, 0);
            let result = ToolResult {
                success: false,
                output: String::new(),
                diff: None,
                error: Some(message),
                stop_reason: None,
            };
            record_governance_after(state, action, &result, model_turn, false);
            GovernancePreflightOutcome::Rejected(result)
        }
        GovernanceDecision::Terminate(reason) => {
            write_event(
                &state.run_id,
                "governance_terminate",
                &format!("{reason:?}"),
                0,
            );
            GovernancePreflightOutcome::Terminate(reason)
        }
    }
}

fn record_governance_after(
    state: &mut AgentState,
    action: &Action,
    result: &ToolResult,
    model_turn: u32,
    read_cache_hit: bool,
) {
    let progress = classify_progress(action, result, read_cache_hit);
    state
        .governance
        .after_action(action, result, progress, model_turn);
}

fn objective_result_fingerprint(action: &Action, result: &ToolResult) -> u64 {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_string(action)
        .unwrap_or_else(|_| format!("{action:?}"))
        .hash(&mut hasher);
    result.success.hash(&mut hasher);
    result.output.hash(&mut hasher);
    result.error.hash(&mut hasher);
    hasher.finish()
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
        Action::Locate { path, .. } => validate_rel_path("locate", path),
        Action::FindFiles { path, .. } => validate_rel_path("find_files", path),
        Action::DetectProjectSnapshot { path } => {
            validate_rel_path("detect_project_snapshot", path)
        }
        Action::SummarizeRepo { path, .. } => validate_rel_path("summarize_repo", path),
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
        assert!(is_parallel_readonly(&Action::Locate {
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
