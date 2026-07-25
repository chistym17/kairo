use serde_json::{json, Value};

use crate::context::compressor::compress_tool_output;
use crate::context::normalize::normalize_rel_path;
use crate::session::Message;
use crate::state::step::{Action, Step};
use crate::tools::ToolResult;

pub fn build_messages(
    system_prompt: &str,
    session_messages: &[Message],
    goal: &str,
    run_summary: Option<&str>,
    step_history: &[Step],
) -> Value {
    let mut messages = vec![json!({
        "role": "system",
        "content": system_prompt
    })];

    for msg in session_messages {
        messages.push(session_message_to_api(msg));
    }

    messages.push(json!({
        "role": "user",
        "content": goal
    }));

    if let Some(summary) = run_summary {
        if !summary.trim().is_empty() {
            messages.push(json!({
                "role": "assistant",
                "content": summary
            }));
        }
    }

    messages.extend(steps_to_messages(step_history));

    json!(messages)
}

/// Convert step history into assistant tool_call + tool result messages.
pub fn steps_to_messages(step_history: &[Step]) -> Vec<Value> {
    let mut messages = Vec::new();
    let mut i = 0;
    while i < step_history.len() {
        let turn = step_history[i].model_turn;
        let mut j = i + 1;
        while j < step_history.len() && step_history[j].model_turn == turn {
            j += 1;
        }
        let group = &step_history[i..j];
        messages.push(steps_to_assistant_message(group));
        for step in group {
            messages.push(tool_result_to_message(
                &step.action,
                &step.result,
                &step.tool_call_id,
            ));
        }
        i = j;
    }
    messages
}

pub fn session_message_to_api(msg: &Message) -> Value {
    match msg {
        Message::User { content } => json!({
            "role": "user",
            "content": content
        }),
        Message::Assistant { content } => json!({
            "role": "assistant",
            "content": content
        }),
        Message::Tool { name, result } => json!({
            "role": "tool",
            "content": compress_tool_output(&format!("[{name}] {result}"))
        }),
        Message::Summary { content } => json!({
            "role": "assistant",
            "content": format!("Session summary:\n{content}")
        }),
    }
}

fn steps_to_assistant_message(steps: &[Step]) -> Value {
    let tool_calls: Vec<Value> = steps
        .iter()
        .map(|step| {
            let (tool_name, args) = action_to_tool_call_parts(&step.action);
            json!({
                "id": step.tool_call_id,
                "type": "function",
                "function": {
                    "name": tool_name,
                    "arguments": serde_json::to_string(&args).unwrap_or_default()
                }
            })
        })
        .collect();

    json!({
        "role": "assistant",
        "content": null,
        "tool_calls": tool_calls
    })
}

fn tool_result_to_message(_action: &Action, result: &ToolResult, tool_call_id: &str) -> Value {
    let content = if result.success {
        let mut parts = vec![result.output.clone()];
        if let Some(diff) = &result.diff {
            if !diff.is_empty() {
                parts.push(format!("\nDiff:\n{diff}"));
            }
        }
        compress_tool_output(&parts.join(""))
    } else {
        let mut parts = vec![];
        if !result.output.is_empty() {
            parts.push(result.output.clone());
        }
        if let Some(err) = &result.error {
            parts.push(format!("Error: {err}"));
        }
        if parts.is_empty() {
            "Tool call failed with no output.".to_string()
        } else {
            compress_tool_output(&parts.join("\n"))
        }
    };

    json!({
        "role": "tool",
        "tool_call_id": tool_call_id,
        "content": content
    })
}

fn action_to_tool_call_parts(action: &Action) -> (&'static str, Value) {
    match action {
        Action::ReadFile {
            path,
            offset,
            limit,
        } => (
            "read_file",
            json!({
                "path": normalize_rel_path(path),
                "offset": offset.to_string(),
                "limit": limit.to_string(),
            }),
        ),
        Action::ListFiles { path } => ("list_files", json!({ "path": normalize_rel_path(path) })),
        Action::SearchCode { query, path } => (
            "search_code",
            json!({
                "query": query,
                "path": normalize_rel_path(path)
            }),
        ),
        Action::Locate { query, path } => (
            "locate",
            json!({
                "query": query,
                "path": normalize_rel_path(path)
            }),
        ),
        Action::FindFiles { pattern, path } => (
            "find_files",
            json!({
                "pattern": pattern,
                "path": normalize_rel_path(path)
            }),
        ),
        Action::DetectProjectSnapshot { path } => (
            "detect_project_snapshot",
            json!({ "path": normalize_rel_path(path) }),
        ),
        Action::SummarizeRepo { depth, path, focus } => (
            "summarize_repo",
            json!({
                "depth": depth,
                "path": normalize_rel_path(path),
                "focus": focus,
            }),
        ),
        Action::GitStatus => ("git_status", json!({})),
        Action::GitDiff { path } => {
            let mut args = json!({});
            if let Some(p) = path {
                args["path"] = json!(normalize_rel_path(p));
            }
            ("git_diff", args)
        }
        Action::WriteFile { path, content } => (
            "write_file",
            json!({
                "path": normalize_rel_path(path),
                "content": content
            }),
        ),
        Action::ApplyPatch { path, diff } => (
            "apply_patch",
            json!({
                "path": normalize_rel_path(path),
                "diff": diff
            }),
        ),
        Action::RunCommand {
            cmd,
            cwd,
            timeout_secs,
        } => (
            "run_command",
            json!({
                "cmd": cmd,
                "working_directory": normalize_rel_path(cwd),
                "timeout_secs": timeout_secs.to_string(),
            }),
        ),
        Action::RunShell {
            command,
            cwd,
            timeout_secs,
        } => (
            "run_shell",
            json!({
                "command": command,
                "working_directory": normalize_rel_path(cwd),
                "timeout_secs": timeout_secs.to_string(),
            }),
        ),
        Action::UpdatePlan { objective, steps } => (
            "update_plan",
            json!({
                "objective": objective,
                "steps": steps,
            }),
        ),
        Action::Finish { summary } => ("finish", json!({ "summary": summary })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::ToolResult;
    use std::path::PathBuf;

    fn ok_result(out: &str) -> ToolResult {
        ToolResult {
            success: true,
            output: out.to_string(),
            diff: None,
            error: None,
            stop_reason: None,
        }
    }

    #[test]
    fn groups_same_model_turn_into_one_assistant_message() {
        let s0 = Step::new(
            0,
            0,
            Action::SearchCode {
                query: "a".into(),
                path: PathBuf::from("."),
            },
            ok_result("hit a"),
            "call_a",
        );
        let s1 = Step::new(
            1,
            0,
            Action::ReadFile {
                path: PathBuf::from("x.rs"),
                offset: 1,
                limit: 50,
            },
            ok_result("code"),
            "call_b",
        );
        let s2 = Step::new(
            2,
            1,
            Action::GitStatus,
            ok_result("clean"),
            "call_c",
        );

        let messages = build_messages("sys", &[], "goal", None, &[s0, s1, s2]);
        let arr = messages.as_array().unwrap();
        // system, user, assistant(batch2), tool, tool, assistant(1), tool
        assert_eq!(arr.len(), 7);
        assert_eq!(arr[2]["role"], "assistant");
        assert_eq!(arr[2]["tool_calls"].as_array().unwrap().len(), 2);
        assert_eq!(arr[3]["tool_call_id"], "call_a");
        assert_eq!(arr[4]["tool_call_id"], "call_b");
        assert_eq!(arr[5]["tool_calls"].as_array().unwrap().len(), 1);
    }
}
