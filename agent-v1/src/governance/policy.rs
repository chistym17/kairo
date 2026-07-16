use std::collections::HashSet;

use crate::state::step::Action;

use super::fingerprint::{is_explore_action, verification_family_for_command, VerificationFamily};
use super::state::{GovernanceState, RuntimeMode};

/// Which tool schemas to expose to the model for the next request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolPolicy {
    pub allowed_tools: HashSet<&'static str>,
    pub blocked_verification_families: HashSet<VerificationFamily>,
    pub mode: RuntimeMode,
}

impl ToolPolicy {
    pub fn from_state(state: &GovernanceState) -> Self {
        let mut allowed: HashSet<&'static str> = ALL_TOOLS.iter().copied().collect();
        let blocked_verification_families = state.blocked_verification_families.clone();

        if state.mode == RuntimeMode::ReviewOrFinish {
            allowed.retain(|t| REVIEW_OR_FINISH_TOOLS.contains(t));
            // run_command / run_shell stay available for non-explore verify only;
            // post-hoc rejection still applies in before_action.
        }

        Self {
            allowed_tools: allowed,
            blocked_verification_families,
            mode: state.mode,
        }
    }

    pub fn allows_tool_name(&self, name: &str) -> bool {
        self.allowed_tools.contains(name)
    }

    pub fn allows_action(&self, action: &Action) -> bool {
        let name = tool_name(action);
        if !self.allows_tool_name(name) {
            return false;
        }
        if self.mode == RuntimeMode::ReviewOrFinish && is_explore_action(action) {
            return false;
        }
        if let Action::RunCommand { cmd, .. } | Action::RunShell { command: cmd, .. } = action {
            if let Some(family) = verification_family_for_command(cmd) {
                if self.blocked_verification_families.contains(&family) {
                    return false;
                }
            }
        }
        true
    }
}

pub const ALL_TOOLS: &[&str] = &[
    "read_file",
    "list_files",
    "search_code",
    "find_files",
    "detect_project_snapshot",
    "git_status",
    "git_diff",
    "write_file",
    "apply_patch",
    "run_command",
    "run_shell",
    "update_plan",
    "finish",
];

const REVIEW_OR_FINISH_TOOLS: &[&str] = &[
    "apply_patch",
    "write_file",
    "git_diff",
    "git_status",
    "run_command",
    "run_shell",
    "update_plan",
    "finish",
];

pub fn tool_name(action: &Action) -> &'static str {
    match action {
        Action::ReadFile { .. } => "read_file",
        Action::ListFiles { .. } => "list_files",
        Action::SearchCode { .. } => "search_code",
        Action::FindFiles { .. } => "find_files",
        Action::DetectProjectSnapshot { .. } => "detect_project_snapshot",
        Action::GitStatus => "git_status",
        Action::GitDiff { .. } => "git_diff",
        Action::WriteFile { .. } => "write_file",
        Action::ApplyPatch { .. } => "apply_patch",
        Action::RunCommand { .. } => "run_command",
        Action::RunShell { .. } => "run_shell",
        Action::UpdatePlan { .. } => "update_plan",
        Action::Finish { .. } => "finish",
    }
}
