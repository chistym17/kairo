use crate::context::{ContextBuilder, ContextManager};
use crate::llm::{LlmClient, LlmError};
use crate::logging::{write_entry, write_event, write_run_summary, LogEntry};
use crate::session::Session;
use crate::state::agent_state::AgentState;
use crate::state::step::Step;
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
        let tool_call = 'next_action: loop {
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

            match llm.next_action(&request).await
            {
                Ok(tool_call) => {
                    println!("[agent] action: {:?}", tool_call.action);
                    break 'next_action tool_call;
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

                        match llm.next_action(&request).await
                        {
                            Ok(tool_call) => {
                                println!(
                                    "[agent] action (after tool_use_failed retry {}): {:?}",
                                    attempt, tool_call.action
                                );
                                break 'next_action tool_call;
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

        let action = tool_call.action;
        let tool_call_id = tool_call.tool_call_id;

        // ── 3. Execute the tool ─────────────────────────────────────────
        emit(
            observer,
            AgentEvent::ToolStarted {
                run_id: state.run_id.clone(),
                step_index: state.step_count,
                tool: tool_name(&action).to_string(),
            },
        );

        let mut result = match validate_action(&action, &state.history, require_verified_finish) {
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
            None => match &action {
                crate::state::step::Action::UpdatePlan { objective, steps } => {
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
                crate::state::step::Action::Finish { summary } => {
                    let verified = finish_allowed(&state.history);
                    let stop_reason = if verified {
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
                _ => tools::dispatch(&action, &state.workspace_root),
            },
        };

        if result.success {
            if let Some(path) = mutating_path(&action) {
                if let Some(err) = post_edit_sanity_check(&state.workspace_root, &path) {
                    write_event(&state.run_id, "post_edit_sanity_failed", &err, 0);
                    result.success = false;
                    result.error = Some(err);
                }
            }
        }

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

        if let crate::state::step::Action::RunCommand { cmd, .. }
        | crate::state::step::Action::RunShell {
            command: cmd,
            ..
        } = &action
        {
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
        let step = Step::new(state.step_count, action, result, tool_call_id);

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
        Action::ListFiles { .. } => "list_files",
        Action::SearchCode { .. } => "search_code",
        Action::FindFiles { .. } => "find_files",
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
    match &state.plan {
        Some(plan) => format!(
            "{}\n\n## Current plan\n{}",
            state.goal,
            plan.render()
        ),
        None => state.goal.clone(),
    }
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
    history: &[Step],
    require_verified_finish: bool,
) -> Option<ValidationError> {
    use crate::state::step::Action;

    match action {
        Action::ReadFile { path, .. } => validate_rel_path("read_file", path),
        Action::ListFiles { path } => validate_rel_path("list_files", path),
        Action::SearchCode { path, .. } => validate_rel_path("search_code", path),
        Action::FindFiles { path, .. } => validate_rel_path("find_files", path),
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
            if require_verified_finish && !finish_allowed(history) {
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
    write_run_summary(
        &state.run_id,
        &state.model_id,
        &reason,
        state.step_count,
    );
    reason
}
