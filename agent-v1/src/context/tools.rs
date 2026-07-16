use serde_json::Value;

use crate::governance::ToolPolicy;
use crate::llm::schema::build_tools_schema;

pub fn build_tools() -> Value {
    build_tools_schema()
}

pub fn build_tools_for_policy(policy: &ToolPolicy) -> Value {
    let full = build_tools_schema();
    let Some(arr) = full.as_array() else {
        return full;
    };
    let filtered: Vec<Value> = arr
        .iter()
        .filter(|t| {
            t.get("function")
                .and_then(|f| f.get("name"))
                .and_then(|n| n.as_str())
                .is_some_and(|name| policy.allows_tool_name(name))
        })
        .cloned()
        .collect();
    Value::Array(filtered)
}
