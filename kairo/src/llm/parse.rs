use serde_json::Value;

use crate::llm::schema::parse_tool_call;
use crate::llm::{LlmError, LlmToolCall};

/// Soft cap so one model turn cannot flood context.
pub const MAX_TOOL_CALLS_PER_TURN: usize = 8;

/// Parse all `tool_calls` from an OpenAI-style assistant message.
pub fn parse_tool_calls_message(message: &Value) -> Result<Vec<LlmToolCall>, LlmError> {
    let tool_calls = message["tool_calls"].as_array().ok_or_else(|| {
        LlmError::MalformedToolCall("tool_calls is not an array".to_string())
    })?;

    if tool_calls.is_empty() {
        return Err(LlmError::MalformedToolCall(
            "tool_calls array is empty".to_string(),
        ));
    }

    let mut out = Vec::with_capacity(tool_calls.len().min(MAX_TOOL_CALLS_PER_TURN));
    for (idx, tool_call) in tool_calls.iter().enumerate() {
        if idx >= MAX_TOOL_CALLS_PER_TURN {
            break;
        }

        let tool_call_id = tool_call["id"]
            .as_str()
            .unwrap_or("call_missing")
            .to_string();

        let tool_name = tool_call["function"]["name"]
            .as_str()
            .ok_or_else(|| LlmError::MalformedToolCall("missing function.name".to_string()))?;

        let args_str = tool_call["function"]["arguments"].as_str().ok_or_else(|| {
            LlmError::MalformedToolCall("missing function.arguments".to_string())
        })?;

        let args: Value = serde_json::from_str(args_str).map_err(|e| {
            let raw = if args_str.len() > 240 {
                format!("{}…", &args_str[..240])
            } else {
                args_str.to_string()
            };
            LlmError::MalformedToolCall(format!(
                "tool `{tool_name}`: arguments is not valid JSON: {e} — raw: {raw}"
            ))
        })?;

        let action = parse_tool_call(tool_name, &args).map_err(|e| {
            LlmError::MalformedToolCall(format!("tool `{tool_name}`: {e}"))
        })?;
        out.push(LlmToolCall {
            action,
            tool_call_id,
        });
    }

    Ok(out)
}

/// True when the choice should be treated as tool-call content.
pub fn message_has_tool_calls(choice: &Value, message: &Value) -> bool {
    let finish_reason = choice["finish_reason"].as_str().unwrap_or("");
    finish_reason == "tool_calls" || message["tool_calls"].is_array()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_multiple_tool_calls() {
        let message = json!({
            "role": "assistant",
            "content": null,
            "tool_calls": [
                {
                    "id": "call_a",
                    "type": "function",
                    "function": {
                        "name": "search_code",
                        "arguments": "{\"query\":\"Bearer\",\"path\":\".\"}"
                    }
                },
                {
                    "id": "call_b",
                    "type": "function",
                    "function": {
                        "name": "git_status",
                        "arguments": "{}"
                    }
                }
            ]
        });

        let calls = parse_tool_calls_message(&message).unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].tool_call_id, "call_a");
        assert_eq!(calls[1].tool_call_id, "call_b");
        assert!(matches!(
            calls[0].action,
            crate::state::step::Action::SearchCode { .. }
        ));
        assert!(matches!(
            calls[1].action,
            crate::state::step::Action::GitStatus
        ));
    }

    #[test]
    fn caps_tool_calls_per_turn() {
        let mut tool_calls = Vec::new();
        for i in 0..(MAX_TOOL_CALLS_PER_TURN + 3) {
            tool_calls.push(json!({
                "id": format!("call_{i}"),
                "type": "function",
                "function": {
                    "name": "git_status",
                    "arguments": "{}"
                }
            }));
        }
        let message = json!({
            "role": "assistant",
            "content": null,
            "tool_calls": tool_calls
        });

        let calls = parse_tool_calls_message(&message).unwrap();
        assert_eq!(calls.len(), MAX_TOOL_CALLS_PER_TURN);
    }

    #[test]
    fn malformed_json_arguments_include_tool_name() {
        let message = json!({
            "role": "assistant",
            "content": null,
            "tool_calls": [
                {
                    "id": "call_bad",
                    "type": "function",
                    "function": {
                        "name": "update_plan",
                        "arguments": "{\"objective\": \"x\", \"steps\": [ ] Explore"
                    }
                }
            ]
        });
        match parse_tool_calls_message(&message) {
            Err(LlmError::MalformedToolCall(msg)) => {
                assert!(msg.contains("tool `update_plan`"), "{msg}");
                assert!(msg.contains("not valid JSON"), "{msg}");
            }
            other => panic!("expected MalformedToolCall, got {other:?}"),
        }
    }
}
