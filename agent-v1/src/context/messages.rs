use serde_json::{json, Value};

use crate::context::compressor::compress_tool_output;
use crate::session::Message;
use crate::state::step::{Action, Step};
use crate::tools::ToolResult;

pub fn build_messages(
    system_prompt: &str,
    session_messages: &[Message],
    goal: &str,
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

    for step in step_history {
        messages.push(action_to_assistant_message(&step.action, &step.tool_call_id));
        messages.push(tool_result_to_message(
            &step.action,
            &step.result,
            &step.tool_call_id,
        ));
    }

    json!(messages)
}

fn session_message_to_api(msg: &Message) -> Value {
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

fn action_to_assistant_message(action: &Action, tool_call_id: &str) -> Value {
    let (tool_name, args) = action_to_tool_call_parts(action);

    json!({
        "role": "assistant",
        "content": null,
        "tool_calls": [{
            "id": tool_call_id,
            "type": "function",
            "function": {
                "name": tool_name,
                "arguments": serde_json::to_string(&args).unwrap_or_default()
            }
        }]
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
        Action::ReadFile { path } => ("read_file", json!({ "path": path.to_string_lossy() })),
        Action::ListFiles { path } => ("list_files", json!({ "path": path.to_string_lossy() })),
        Action::SearchCode { query, path } => (
            "search_code",
            json!({
                "query": query,
                "path": path.to_string_lossy()
            }),
        ),
        Action::FindFiles { pattern, path } => (
            "find_files",
            json!({
                "pattern": pattern,
                "path": path.to_string_lossy()
            }),
        ),
        Action::GitStatus => ("git_status", json!({})),
        Action::GitDiff { path } => {
            let mut args = json!({});
            if let Some(p) = path {
                args["path"] = json!(p.to_string_lossy());
            }
            ("git_diff", args)
        }
        Action::WriteFile { path, content } => (
            "write_file",
            json!({
                "path": path.to_string_lossy(),
                "content": content
            }),
        ),
        Action::ApplyPatch { path, diff } => (
            "apply_patch",
            json!({
                "path": path.to_string_lossy(),
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
                "working_directory": cwd.to_string_lossy(),
                "timeout_secs": timeout_secs.to_string(),
            }),
        ),
        Action::Finish { summary } => ("finish", json!({ "summary": summary })),
    }
}
