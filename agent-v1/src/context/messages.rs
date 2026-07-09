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
        messages.push(session_message_to_groq(msg));
    }

    messages.push(json!({
        "role": "user",
        "content": goal
    }));

    for step in step_history {
        messages.push(action_to_assistant_message(&step.action));
        messages.push(tool_result_to_message(&step.action, &step.result));
    }

    json!(messages)
}

fn session_message_to_groq(msg: &Message) -> Value {
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

fn action_to_assistant_message(action: &Action) -> Value {
    let (tool_name, args) = action_to_tool_call_parts(action);

    json!({
        "role": "assistant",
        "content": null,
        "tool_calls": [{
            "id": format!("call_{}", uuid::Uuid::new_v4().to_string().replace('-', "")[..8].to_string()),
            "type": "function",
            "function": {
                "name": tool_name,
                "arguments": serde_json::to_string(&args).unwrap_or_default()
            }
        }]
    })
}

fn tool_result_to_message(action: &Action, result: &ToolResult) -> Value {
    let (tool_name, _) = action_to_tool_call_parts(action);

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
        "tool_call_id": format!("call_{}", &tool_name[..4.min(tool_name.len())]),
        "content": content
    })
}

fn action_to_tool_call_parts(action: &Action) -> (&'static str, Value) {
    match action {
        Action::ReadFile { path } => ("read_file", json!({ "path": path.to_string_lossy() })),
        Action::ListFiles {
            path,
            max_depth,
            max_entries,
        } => (
            "list_files",
            json!({
                "path": path.to_string_lossy(),
                "max_depth": max_depth,
                "max_entries": max_entries
            }),
        ),
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
        Action::RunCommand { cmd } => ("run_command", json!({ "cmd": cmd })),
        Action::Finish { summary } => ("finish", json!({ "summary": summary })),
    }
}
