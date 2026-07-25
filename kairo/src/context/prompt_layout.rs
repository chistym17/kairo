//! Stable vs dynamic prompt layout + fingerprints (Phase 13A Day 2–3).
//!
//! Diagnostics are for logs only — never inject hashes into the model prompt.
//!
//! Stable prefix must not include: timestamps, run IDs, usage/cache stats,
//! approval counters, or unordered map serialization.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::context::messages::{session_message_to_api, steps_to_messages};
use crate::context::normalize::{canonicalize_json, canonicalize_json_string, normalize_text};
use crate::session::Message;
use crate::state::step::Step;

/// Fingerprints for one request. Log/event only — not part of the LLM payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PromptDiagnostics {
    pub stable_prefix_hash: String,
    pub stable_prefix_bytes: usize,
    pub tool_schema_hash: String,
    pub system_prompt_hash: String,
    pub project_instructions_hash: Option<String>,
    /// Hash of stable messages after the system message (session history).
    pub session_messages_hash: String,
}

impl PromptDiagnostics {
    pub fn render_compact(&self) -> String {
        format!(
            "stable={} bytes={} tools={} system={} instructions={} session={}",
            &self.stable_prefix_hash[..self.stable_prefix_hash.len().min(8)],
            self.stable_prefix_bytes,
            &self.tool_schema_hash[..self.tool_schema_hash.len().min(8)],
            &self.system_prompt_hash[..self.system_prompt_hash.len().min(8)],
            self.project_instructions_hash
                .as_ref()
                .map(|h| h[..h.len().min(8)].to_string())
                .unwrap_or_else(|| "none".to_string()),
            &self.session_messages_hash[..self.session_messages_hash.len().min(8)],
        )
    }
}

/// Explicit split used to keep the provider-cacheable prefix byte-stable.
#[derive(Debug, Clone)]
pub struct PromptLayout {
    pub tools: Value,
    pub base_system_prompt: String,
    pub project_instructions: Option<String>,
    pub composed_system_prompt: String,
    /// system + prior session messages (unchanged when only goal/steps change)
    pub stable_messages: Vec<Value>,
    /// current goal, optional run summary, step history
    pub dynamic_messages: Vec<Value>,
}

impl PromptLayout {
    pub fn assemble_messages(&self) -> Value {
        let mut all = self.stable_messages.clone();
        all.extend(self.dynamic_messages.iter().cloned());
        // Canonical key order so the wire JSON does not churn.
        canonicalize_json(&Value::Array(all))
    }

    pub fn stable_prefix_bytes(&self) -> usize {
        stable_prefix_payload(&self.tools, &self.stable_messages).len()
    }

    pub fn stable_prefix_hash(&self) -> String {
        hash_hex(&stable_prefix_payload(&self.tools, &self.stable_messages))
    }

    pub fn diagnostics(&self) -> PromptDiagnostics {
        let session_only: Vec<Value> = self.stable_messages.iter().skip(1).cloned().collect();
        PromptDiagnostics {
            stable_prefix_hash: self.stable_prefix_hash(),
            stable_prefix_bytes: self.stable_prefix_bytes(),
            tool_schema_hash: hash_hex(canonicalize_json_string(&self.tools).as_bytes()),
            system_prompt_hash: hash_hex(self.base_system_prompt.as_bytes()),
            project_instructions_hash: self
                .project_instructions
                .as_ref()
                .filter(|s| !s.trim().is_empty())
                .map(|s| hash_hex(s.as_bytes())),
            session_messages_hash: hash_hex(
                canonicalize_json_string(&Value::Array(session_only)).as_bytes(),
            ),
        }
    }
}

fn stable_prefix_payload(tools: &Value, stable_messages: &[Value]) -> Vec<u8> {
    let tools_s = canonicalize_json_string(tools);
    let msgs_s = canonicalize_json_string(&Value::Array(stable_messages.to_vec()));
    let mut out = Vec::with_capacity(tools_s.len() + msgs_s.len() + 1);
    out.extend_from_slice(tools_s.as_bytes());
    out.push(b'\n');
    out.extend_from_slice(msgs_s.as_bytes());
    out
}

/// FNV-1a 64-bit — deterministic across processes (unlike `DefaultHasher`).
pub fn hash_hex(data: &[u8]) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in data {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Sort tools by function name, then canonicalize object keys.
pub fn sort_tools_by_name(tools: Value) -> Value {
    let Some(mut arr) = tools.as_array().cloned() else {
        return canonicalize_json(&tools);
    };
    arr.sort_by(|a, b| tool_name(a).cmp(&tool_name(b)));
    canonicalize_json(&Value::Array(arr))
}

fn tool_name(tool: &Value) -> String {
    tool.get("function")
        .and_then(|f| f.get("name"))
        .and_then(|n| n.as_str())
        .unwrap_or("")
        .to_string()
}

/// Build layout: tools + system (+ instructions) + session are stable;
/// goal / run_summary / steps are dynamic.
pub fn build_prompt_layout(
    base_system_prompt: &str,
    project_instructions: Option<&str>,
    composed_system_prompt: &str,
    tools: Value,
    session_messages: &[Message],
    goal: &str,
    run_summary: Option<&str>,
    step_history: &[Step],
) -> PromptLayout {
    let tools = sort_tools_by_name(tools);
    let base_system_prompt = normalize_text(base_system_prompt);
    let project_instructions = project_instructions
        .map(normalize_text)
        .filter(|s| !s.is_empty());
    let composed_system_prompt = normalize_text(composed_system_prompt);

    let mut stable_messages = vec![json!({
        "role": "system",
        "content": composed_system_prompt
    })];
    for msg in session_messages {
        stable_messages.push(canonicalize_json(&session_message_to_api(msg)));
    }

    let mut dynamic_messages = vec![json!({
        "role": "user",
        "content": goal
    })];

    if let Some(summary) = run_summary {
        if !summary.trim().is_empty() {
            dynamic_messages.push(json!({
                "role": "assistant",
                "content": summary
            }));
        }
    }

    dynamic_messages.extend(steps_to_messages(step_history));
    dynamic_messages = dynamic_messages
        .into_iter()
        .map(|m| canonicalize_json(&m))
        .collect();

    PromptLayout {
        tools,
        base_system_prompt,
        project_instructions,
        composed_system_prompt,
        stable_messages,
        dynamic_messages,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::system::default_system_prompt;
    use crate::context::tools::build_tools;
    use crate::context::ContextBuilder;
    use crate::llm::schema::build_tools_schema;
    use crate::state::step::Action;
    use crate::tools::ToolResult;

    fn layout_with(
        tools: Value,
        session: &[Message],
        goal: &str,
        steps: &[Step],
    ) -> PromptLayout {
        let base = default_system_prompt();
        build_prompt_layout(&base, None, &base, tools, session, goal, None, steps)
    }

    #[test]
    fn identical_state_same_stable_prefix_bytes() {
        let tools = build_tools();
        let first = layout_with(tools.clone(), &[], "goal", &[]);
        let second = layout_with(tools, &[], "goal", &[]);
        assert_eq!(first.stable_prefix_bytes(), second.stable_prefix_bytes());
        assert_eq!(first.stable_prefix_hash(), second.stable_prefix_hash());
        assert_eq!(first.diagnostics(), second.diagnostics());
        assert_eq!(
            serde_json::to_string(&first.tools).unwrap(),
            serde_json::to_string(&second.tools).unwrap()
        );
        assert_eq!(
            serde_json::to_string(&first.stable_messages).unwrap(),
            serde_json::to_string(&second.stable_messages).unwrap()
        );
    }

    #[test]
    fn latest_user_message_does_not_change_stable_prefix() {
        let tools = build_tools();
        let first = layout_with(tools.clone(), &[], "goal a", &[]);
        let changed = layout_with(tools, &[], "goal b — different", &[]);
        assert_eq!(first.stable_prefix_hash(), changed.stable_prefix_hash());
        assert_eq!(first.stable_prefix_bytes(), changed.stable_prefix_bytes());
        assert_ne!(
            serde_json::to_string(&first.assemble_messages()).unwrap(),
            serde_json::to_string(&changed.assemble_messages()).unwrap()
        );
    }

    #[test]
    fn tool_schema_change_changes_stable_prefix() {
        let tools = build_tools();
        let first = layout_with(tools, &[], "goal", &[]);

        let mut altered = build_tools_schema();
        if let Some(arr) = altered.as_array_mut() {
            if let Some(first_tool) = arr.first_mut() {
                first_tool["function"]["description"] =
                    json!("CHANGED DESCRIPTION FOR HASH TEST");
            }
        }
        let changed = layout_with(altered, &[], "goal", &[]);
        assert_ne!(first.stable_prefix_hash(), changed.stable_prefix_hash());
        assert_ne!(
            first.diagnostics().tool_schema_hash,
            changed.diagnostics().tool_schema_hash
        );
    }

    #[test]
    fn tools_are_sorted_by_name() {
        let tools = sort_tools_by_name(build_tools_schema());
        let names: Vec<_> = tools
            .as_array()
            .unwrap()
            .iter()
            .map(|t| tool_name(t))
            .collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
    }

    #[test]
    fn step_history_is_dynamic_only() {
        let step = Step::new(
            0,
            0,
            Action::GitStatus,
            ToolResult {
                success: true,
                output: "ok".into(),
                error: None,
                diff: None,
                stop_reason: None,
            },
            "call_1",
        );
        let tools = build_tools();
        let without = layout_with(tools.clone(), &[], "goal", &[]);
        let with_step = layout_with(tools, &[], "goal", &[step]);
        assert_eq!(without.stable_prefix_hash(), with_step.stable_prefix_hash());
        assert!(with_step.dynamic_messages.len() > without.dynamic_messages.len());
    }

    #[test]
    fn tool_key_order_does_not_change_schema_hash() {
        let a = json!([{
            "type": "function",
            "function": {
                "name": "zzz",
                "description": "d",
                "parameters": {"type": "object", "properties": {}}
            }
        }]);
        let b = json!([{
            "function": {
                "parameters": {"properties": {}, "type": "object"},
                "description": "d",
                "name": "zzz"
            },
            "type": "function"
        }]);
        let la = layout_with(a, &[], "g", &[]);
        let lb = layout_with(b, &[], "g", &[]);
        assert_eq!(
            la.diagnostics().tool_schema_hash,
            lb.diagnostics().tool_schema_hash
        );
        assert_eq!(la.stable_prefix_hash(), lb.stable_prefix_hash());
    }

    #[test]
    fn agents_line_endings_do_not_churn_prefix() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "rule A\r\nrule B\r\n").unwrap();
        let a = ContextBuilder::new().build(dir.path(), "goal", &[], &[]);
        std::fs::write(dir.path().join("AGENTS.md"), "rule A\nrule B\n").unwrap();
        let b = ContextBuilder::new().build(dir.path(), "goal", &[], &[]);
        assert_eq!(
            a.diagnostics.stable_prefix_hash,
            b.diagnostics.stable_prefix_hash
        );
        assert_eq!(
            a.diagnostics.project_instructions_hash,
            b.diagnostics.project_instructions_hash
        );
    }
}
