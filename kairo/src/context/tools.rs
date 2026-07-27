use serde_json::Value;

use crate::context::prompt_layout::sort_tools_by_name;
use crate::governance::ToolPolicy;
use crate::llm::schema::build_tools_schema;
use crate::state::phase::Phase;

pub fn build_tools() -> Value {
    sort_tools_by_name(build_tools_schema())
}

/// Tool schemas for an explicit allow-list (phase or patch-recovery).
///
/// Unlike semantic `ToolPolicy` (execution gate only), this **removes** tools
/// from the model request so Explore → Act / recovery actually changes capabilities.
pub fn tools_for_allowed(allowed: &[&str]) -> Value {
    sort_tools_by_name(filter_tools_by_names(build_tools_schema(), allowed))
}

/// Tool schemas for the current runtime phase (slice 2).
pub fn tools_for_phase(phase: Phase) -> Value {
    tools_for_allowed(phase.allowed_tools())
}

/// Intersect allowed tools with an optional semantic policy allow-list.
pub fn build_tools_for_allowed(allowed: &[&str], policy: Option<&ToolPolicy>) -> Value {
    let mut tools = tools_for_allowed(allowed);
    if let Some(policy) = policy {
        let names: Vec<&str> = policy.allowed_tools.iter().copied().collect();
        tools = sort_tools_by_name(filter_tools_by_names(tools, &names));
    }
    tools
}

/// Intersect phase tools with an optional semantic policy allow-list.
pub fn build_tools_for_phase(phase: Phase, policy: Option<&ToolPolicy>) -> Value {
    build_tools_for_allowed(phase.allowed_tools(), policy)
}

/// Kept for callers that only have a policy; defaults to Explore (full set).
pub fn build_tools_for_policy(policy: &ToolPolicy) -> Value {
    build_tools_for_phase(Phase::Explore, Some(policy))
}

fn filter_tools_by_names(tools: Value, allowed: &[&str]) -> Value {
    let Some(arr) = tools.as_array() else {
        return tools;
    };
    let filtered: Vec<Value> = arr
        .iter()
        .filter(|tool| {
            tool.get("function")
                .and_then(|f| f.get("name"))
                .and_then(|n| n.as_str())
                .map(|name| allowed.iter().any(|a| *a == name))
                .unwrap_or(false)
        })
        .cloned()
        .collect();
    Value::Array(filtered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::phase::{Phase, RunState, PATCH_RECOVERY_TOOLS};
    use std::path::Path;

    fn tool_names(tools: &Value) -> Vec<String> {
        tools
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| {
                t.get("function")?
                    .get("name")?
                    .as_str()
                    .map(str::to_string)
            })
            .collect()
    }

    #[test]
    fn act_mode_request_excludes_search_and_shell_tools() {
        let tools = tools_for_phase(Phase::Act);
        let names = tool_names(&tools);
        assert_eq!(
            names,
            vec![
                "apply_patch".to_string(),
                "finish".to_string(),
                "read_file".to_string(),
                "update_plan".to_string(),
            ]
        );
        for banned in [
            "search_code",
            "find_files",
            "locate",
            "list_files",
            "run_command",
            "run_shell",
            "git_status",
            "git_diff",
            "write_file",
        ] {
            assert!(
                !names.iter().any(|n| n == banned),
                "Act schema must not include {banned}"
            );
        }
    }

    #[test]
    fn explore_includes_search_and_shell() {
        let names = tool_names(&tools_for_phase(Phase::Explore));
        assert!(names.iter().any(|n| n == "search_code"));
        assert!(names.iter().any(|n| n == "run_shell"));
        assert!(names.iter().any(|n| n == "read_file"));
    }

    #[test]
    fn patch_recovery_request_excludes_plan_search_and_shell() {
        let mut run = RunState::new();
        let _ = run.enter_patch_recovery(Path::new("x.rs"));
        let names = tool_names(&tools_for_allowed(run.effective_tools()));
        assert_eq!(
            names,
            vec![
                "apply_patch".to_string(),
                "finish".to_string(),
                "read_file".to_string(),
            ]
        );
        assert_eq!(run.effective_tools(), PATCH_RECOVERY_TOOLS);
        assert!(!names.iter().any(|n| n == "update_plan"));
        assert!(!names.iter().any(|n| n == "search_code"));
        assert!(!names.iter().any(|n| n == "run_shell"));
    }
}
