use std::path::Path;

use crate::session::Message;
use crate::state::step::Step;

use super::instructions::load_instructions;
use super::messages::build_messages;
use super::stats::ContextStats;
use super::system::{compose_system_prompt, default_system_prompt};
use super::tools::{build_tools, build_tools_for_policy};
use super::types::{AgentContext, LlmRequest, WorkspaceContext};
use crate::governance::ToolPolicy;

/// Assembles the full context sent to the LLM for each decision.
#[derive(Debug, Clone)]
pub struct ContextBuilder {
    system_prompt: String,
}

impl ContextBuilder {
    pub fn new() -> Self {
        Self {
            system_prompt: default_system_prompt(),
        }
    }

    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = prompt.into();
        self
    }

    pub fn system_prompt(&self) -> &str {
        &self.system_prompt
    }

    pub fn build_context(
        &self,
        workspace_root: &Path,
        goal: &str,
        session_messages: &[Message],
        step_history: &[Step],
    ) -> AgentContext {
        self.build_context_with_policy(workspace_root, goal, session_messages, None, step_history, None)
    }

    pub fn build_context_with_policy(
        &self,
        workspace_root: &Path,
        goal: &str,
        session_messages: &[Message],
        run_summary: Option<&str>,
        step_history: &[Step],
        tool_policy: Option<&ToolPolicy>,
    ) -> AgentContext {
        let instructions = load_instructions(workspace_root);
        let system_prompt =
            compose_system_prompt(&self.system_prompt, instructions.as_deref());
        let tools = match tool_policy {
            Some(policy) => build_tools_for_policy(policy),
            None => build_tools(),
        };

        AgentContext {
            system_prompt,
            session_messages: session_messages.to_vec(),
            goal: goal.to_string(),
            run_summary: run_summary.map(str::to_string),
            step_history: step_history.to_vec(),
            tools,
            workspace: WorkspaceContext {
                root: workspace_root.to_path_buf(),
                instructions,
            },
        }
    }

    pub fn build(
        &self,
        workspace_root: &Path,
        goal: &str,
        session_messages: &[Message],
        step_history: &[Step],
    ) -> LlmRequest {
        self.build_with_stats(workspace_root, goal, session_messages, step_history)
            .0
    }

    pub fn build_with_stats(
        &self,
        workspace_root: &Path,
        goal: &str,
        session_messages: &[Message],
        step_history: &[Step],
    ) -> (LlmRequest, ContextStats) {
        self.build_with_stats_and_policy(
            workspace_root,
            goal,
            session_messages,
            None,
            step_history,
            None,
        )
    }

    pub fn build_with_stats_and_policy(
        &self,
        workspace_root: &Path,
        goal: &str,
        session_messages: &[Message],
        run_summary: Option<&str>,
        step_history: &[Step],
        tool_policy: Option<&ToolPolicy>,
    ) -> (LlmRequest, ContextStats) {
        let context = self.build_context_with_policy(
            workspace_root,
            goal,
            session_messages,
            run_summary,
            step_history,
            tool_policy,
        );
        let messages = build_messages(
            &context.system_prompt,
            &context.session_messages,
            &context.goal,
            context.run_summary.as_deref(),
            &context.step_history,
        );

        let request = LlmRequest {
            messages,
            tools: context.tools,
        };

        let stats = ContextStats::from_llm_request(&request.messages, &request.tools);

        (request, stats)
    }
}

impl Default for ContextBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::Message;
    use crate::state::step::Action;
    use crate::tools::ToolResult;

    #[test]
    fn request_includes_system_goal_tools_and_session() {
        let dir = tempfile::tempdir().unwrap();
        let builder = ContextBuilder::new();
        let session = [Message::user("prior bug report")];

        let request = builder.build(dir.path(), "fix divide", &session, &[]);

        let messages = request.messages.as_array().unwrap();
        assert_eq!(messages[0]["role"], "system");
        assert!(messages[0]["content"]
            .as_str()
            .unwrap()
            .contains("coding agent"));
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[1]["content"], "prior bug report");
        assert_eq!(messages[2]["role"], "user");
        assert_eq!(messages[2]["content"], "fix divide");
        assert!(request.tools.as_array().unwrap().len() >= 4);
    }

    #[test]
    fn agents_md_appended_to_system_prompt() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("AGENTS.md"),
            "- Use cargo test\n- Do not modify tests",
        )
        .unwrap();

        let context = ContextBuilder::new().build_context(dir.path(), "goal", &[], &[]);

        assert!(context.system_prompt.contains("Project instructions"));
        assert!(context.system_prompt.contains("cargo test"));
    }

    #[test]
    fn step_history_appended_after_goal() {
        use crate::state::step::Step;

        let dir = tempfile::tempdir().unwrap();
        let step = Step::new(
            0,
            0,
            Action::ReadFile {
                path: "calc.py".into(),
                offset: 1,
                limit: 250,
            },
            ToolResult {
                success: true,
                output: "def add(): pass".to_string(),
                error: None,
                diff: None,
                stop_reason: None,
            },
            "call_test_0",
        );

        let request = ContextBuilder::new().build(dir.path(), "goal", &[], &[step]);
        let messages = request.messages.as_array().unwrap();

        assert_eq!(messages.len(), 4); // system, user goal, assistant tool_call, tool result
        assert_eq!(messages[3]["role"], "tool");
    }

    #[test]
    fn tool_output_is_compressed_in_llm_view() {
        use crate::state::step::Step;

        let dir = tempfile::tempdir().unwrap();
        let big = (0..500).map(|i| format!("line {i}\n")).collect::<String>();

        let step = Step::new(
            0,
            0,
            Action::RunCommand {
                cmd: "pytest".to_string(),
                cwd: std::path::PathBuf::from("."),
                timeout_secs: 30,
            },
            ToolResult {
                success: false,
                output: big,
                error: None,
                diff: None,
                stop_reason: None,
            },
            "call_test_1",
        );

        let request = ContextBuilder::new().build(dir.path(), "goal", &[], &[step]);
        let messages = request.messages.as_array().unwrap();

        let tool_msg = messages.iter().find(|m| m["role"] == "tool").unwrap();
        let content = tool_msg["content"].as_str().unwrap();
        assert!(content.contains("output truncated"));
    }

    #[test]
    fn build_with_stats_returns_stats() {
        let dir = tempfile::tempdir().unwrap();
        let session = [Message::user("prior")];

        let (request, stats) =
            ContextBuilder::new().build_with_stats(dir.path(), "goal", &session, &[]);

        assert_eq!(stats.message_count, request.messages.as_array().unwrap().len());
        assert!(stats.estimated_tokens > 0);
    }
}
