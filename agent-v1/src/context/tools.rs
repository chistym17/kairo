use serde_json::Value;

use crate::context::prompt_layout::sort_tools_by_name;
use crate::governance::ToolPolicy;
use crate::llm::schema::build_tools_schema;

pub fn build_tools() -> Value {
    sort_tools_by_name(build_tools_schema())
}

/// Tool schemas stay the full stable set. Authorization / semantic policy gate
/// execution — they must not reshape the cached tool prefix between turns.
pub fn build_tools_for_policy(_policy: &ToolPolicy) -> Value {
    build_tools()
}
