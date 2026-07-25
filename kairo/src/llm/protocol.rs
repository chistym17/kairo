//! Provider-independent model response protocol.
//!
//! Adapters convert provider wire formats into [`AgentResponse`]. The agent loop
//! should eventually match only on this enum — never on Groq/DeepSeek/etc. specifics.

use serde_json::Value;

use crate::llm::parse::{message_has_tool_calls, parse_tool_calls_message};
use crate::llm::{LlmError, LlmToolCall};

/// Normalized outcome of one model turn (after transport succeeded).
#[derive(Debug, Clone)]
pub enum AgentResponse {
    /// Model requested one or more tool calls.
    ToolCalls(Vec<LlmToolCall>),
    /// Model returned a final text answer (no tools).
    FinalAnswer(String),
    /// Model response was structurally invalid for the agent protocol.
    ProtocolError(ProtocolError),
}

/// Protocol-level failure inside an otherwise successful HTTP response.
#[derive(Debug, Clone)]
pub enum ProtocolError {
    MalformedToolCall(String),
}

impl AgentResponse {
    /// Map to the legacy `next_actions` Result used by the loop engine today.
    pub fn into_llm_result(self) -> Result<Vec<LlmToolCall>, LlmError> {
        match self {
            AgentResponse::ToolCalls(calls) => Ok(calls),
            AgentResponse::FinalAnswer(text) => Err(LlmError::NoToolCall(text)),
            AgentResponse::ProtocolError(ProtocolError::MalformedToolCall(msg)) => {
                Err(LlmError::MalformedToolCall(msg))
            }
        }
    }
}

/// Normalize an OpenAI-compatible chat completion `choices[0]` object.
///
/// Shared by Groq, DeepSeek, and AgentRouter (all OpenAI-style chat APIs).
pub fn normalize_openai_chat_choice(choice: &Value) -> AgentResponse {
    let message = &choice["message"];

    if message_has_tool_calls(choice, message) {
        match parse_tool_calls_message(message) {
            Ok(calls) => AgentResponse::ToolCalls(calls),
            Err(LlmError::MalformedToolCall(msg)) => {
                AgentResponse::ProtocolError(ProtocolError::MalformedToolCall(msg))
            }
            Err(other) => AgentResponse::ProtocolError(ProtocolError::MalformedToolCall(
                other.to_string(),
            )),
        }
    } else {
        AgentResponse::FinalAnswer(final_answer_text(choice, message))
    }
}

fn final_answer_text(choice: &Value, message: &Value) -> String {
    let finish = choice["finish_reason"].as_str().unwrap_or("");
    let content = message["content"].as_str().unwrap_or("").trim();
    let reasoning = message["reasoning_content"]
        .as_str()
        .unwrap_or("")
        .trim();

    let mut text = if !content.is_empty() {
        content.to_string()
    } else if !reasoning.is_empty() {
        format!(
            "[reasoning_only] {}",
            &reasoning[..reasoning.len().min(400)]
        )
    } else {
        "[no content]".to_string()
    };

    if finish == "length" {
        text = format!(
            "[truncated: max_tokens reached with no tool call] {text}\n\
             Next response must be a short tool call (prefer apply_patch/write_file)."
        );
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tool_calls_normalize() {
        let choice = json!({
            "finish_reason": "tool_calls",
            "message": {
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {
                        "name": "search_code",
                        "arguments": "{\"query\":\"foo\",\"path\":\".\"}"
                    }
                }]
            }
        });
        match normalize_openai_chat_choice(&choice) {
            AgentResponse::ToolCalls(calls) => {
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].tool_call_id, "call_1");
            }
            other => panic!("expected ToolCalls, got {:?}", other),
        }
    }

    #[test]
    fn final_answer_normalize() {
        let choice = json!({
            "finish_reason": "stop",
            "message": {
                "role": "assistant",
                "content": "done"
            }
        });
        match normalize_openai_chat_choice(&choice) {
            AgentResponse::FinalAnswer(text) => assert_eq!(text, "done"),
            other => panic!("expected FinalAnswer, got {:?}", other),
        }
    }

    #[test]
    fn malformed_becomes_protocol_error() {
        let choice = json!({
            "finish_reason": "tool_calls",
            "message": {
                "role": "assistant",
                "tool_calls": [{
                    "id": "call_bad",
                    "type": "function",
                    "function": {
                        "name": "search_code",
                        "arguments": "not-json"
                    }
                }]
            }
        });
        match normalize_openai_chat_choice(&choice) {
            AgentResponse::ProtocolError(ProtocolError::MalformedToolCall(_)) => {}
            other => panic!("expected ProtocolError, got {:?}", other),
        }
    }

    #[test]
    fn into_llm_result_preserves_legacy_errors() {
        let final_ans = AgentResponse::FinalAnswer("hi".into());
        assert!(matches!(
            final_ans.into_llm_result(),
            Err(LlmError::NoToolCall(_))
        ));

        let bad = AgentResponse::ProtocolError(ProtocolError::MalformedToolCall("x".into()));
        assert!(matches!(
            bad.into_llm_result(),
            Err(LlmError::MalformedToolCall(_))
        ));
    }

    #[test]
    fn length_truncation_surfaces_in_final_answer() {
        let choice = json!({
            "finish_reason": "length",
            "message": {
                "role": "assistant",
                "content": "",
                "reasoning_content": "planning a large multi-file patch"
            }
        });
        match normalize_openai_chat_choice(&choice) {
            AgentResponse::FinalAnswer(text) => {
                assert!(text.contains("truncated"));
                assert!(text.contains("reasoning_only") || text.contains("planning"));
                assert!(text.contains("apply_patch"));
            }
            other => panic!("expected FinalAnswer, got {:?}", other),
        }
    }
}
