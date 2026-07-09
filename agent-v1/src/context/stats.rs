use serde::Serialize;
use serde_json::Value;

use crate::context::token::{estimate_tokens_from_json, estimate_tokens_from_str};

/// Basic visibility into how large the context is.
///
/// Phase 6.1 only: this is measurement, not enforcement.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ContextStats {
    pub message_count: usize,
    pub estimated_tokens: usize,
    pub tool_output_tokens: usize,
}

impl ContextStats {
    pub fn to_json_string(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }

    pub fn from_llm_request(messages: &Value, tools: &Value) -> Self {
        let message_count = messages.as_array().map(|a| a.len()).unwrap_or(0);

        let messages_tokens = estimate_tokens_from_json(messages);
        let tools_tokens = estimate_tokens_from_json(tools);

        let tool_output_tokens = estimate_tool_output_tokens(messages);

        Self {
            message_count,
            estimated_tokens: messages_tokens + tools_tokens,
            tool_output_tokens,
        }
    }
}

fn estimate_tool_output_tokens(messages: &Value) -> usize {
    let Some(arr) = messages.as_array() else {
        return 0;
    };

    let mut sum = 0usize;
    for msg in arr {
        if msg.get("role").and_then(|r| r.as_str()) != Some("tool") {
            continue;
        }
        if let Some(content) = msg.get("content").and_then(|c| c.as_str()) {
            sum += estimate_tokens_from_str(content);
        }
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn stats_counts_messages_and_tool_tokens() {
        let messages = json!([
            {"role":"system","content":"sys"},
            {"role":"user","content":"hi"},
            {"role":"tool","content":"line1\nline2\nline3"}
        ]);
        let tools = json!([{"type":"function","function":{"name":"read_file"}}]);

        let stats = ContextStats::from_llm_request(&messages, &tools);
        assert_eq!(stats.message_count, 3);
        assert!(stats.estimated_tokens > 0);
        assert!(stats.tool_output_tokens > 0);
    }
}

