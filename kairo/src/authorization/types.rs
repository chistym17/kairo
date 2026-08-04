use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

use crate::state::step::Action;

/// What mutations the runtime permits (safety authority).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum EditPolicy {
    /// Allow all classified effects without prompting.
    #[default]
    Auto,
    /// Pause before the first ordinary workspace mutation (and for
    /// destructive / external effects separately).
    ConfirmBeforeFirstEdit,
    /// Deny every non-read-only effect.
    ReadOnly,
}

impl EditPolicy {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "confirm" | "confirm_before_first_edit" | "confirm-before-first-edit" => {
                Some(Self::ConfirmBeforeFirstEdit)
            }
            "readonly" | "read_only" | "read-only" => Some(Self::ReadOnly),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::ConfirmBeforeFirstEdit => "confirm",
            Self::ReadOnly => "readonly",
        }
    }
}

/// Declared side effect of a tool call.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolEffect {
    ReadOnly,
    WorkspaceMutation,
    DestructiveMutation,
    ExternalSideEffect,
}

/// Coarse scope placeholder (full scope engine deferred).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    #[default]
    Unspecified,
    Paths(Vec<PathBuf>),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApprovalId(pub String);

impl ApprovalId {
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }
}

impl Default for ApprovalId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApprovalRequest {
    pub id: ApprovalId,
    pub effect: ToolEffect,
    pub summary: String,
    pub proposed_scope: Scope,
    pub reason: String,
    /// Reserved for revision-tied expiry; unused in v1.
    pub expires_at_revision: Option<String>,
    /// Proposed patch / file body for UI preview (truncated when large).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    ApproveOnce,
    /// Ordinary workspace mutations for the rest of this task / session grant.
    ApproveForTask,
    Reject,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuthorizationDecision {
    Allow,
    RequireApproval(ApprovalRequest),
    Deny(String),
}

/// Persisted / in-run authorization state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthorizationState {
    pub edit_policy: EditPolicy,
    /// `ApproveForTask` grant — ordinary [`ToolEffect::WorkspaceMutation`] only.
    pub workspace_mutations_approved: bool,
}

impl Default for AuthorizationState {
    fn default() -> Self {
        Self::new(EditPolicy::Auto)
    }
}

impl AuthorizationState {
    pub fn new(edit_policy: EditPolicy) -> Self {
        Self {
            edit_policy,
            workspace_mutations_approved: false,
        }
    }

    /// Clear task-scoped grants (e.g. when the objective changes).
    pub fn revoke_task_approvals(&mut self) {
        self.workspace_mutations_approved = false;
    }
}

/// Classify an action's effect. Thin seam — not a full `Tool` trait yet.
pub fn tool_effect(action: &Action) -> ToolEffect {
    match action {
        Action::ReadFile { .. }
        | Action::ListFiles { .. }
        | Action::SearchCode { .. }
        | Action::Locate { .. }
        | Action::FindFiles { .. }
        | Action::DetectProjectSnapshot { .. }
        | Action::SummarizeRepo { .. }
        | Action::GitStatus
        | Action::GitDiff { .. }
        | Action::UpdatePlan { .. }
        | Action::Finish { .. } => ToolEffect::ReadOnly,

        Action::WriteFile { .. } | Action::ApplyPatch { .. } => ToolEffect::WorkspaceMutation,

        // Explore shells and ordinary verify/test/lint shells are read-only for
        // authorization (no approval spam). Installs / package mutation stay external.
        Action::RunCommand { cmd, .. } => classify_shell_effect(cmd),
        Action::RunShell { command, .. } => classify_shell_effect(command),
    }
}

fn classify_shell_effect(command: &str) -> ToolEffect {
    if crate::tools::shell_cache::is_explore_shell(command) {
        return ToolEffect::ReadOnly;
    }
    if is_package_install_shell(command) {
        return ToolEffect::ExternalSideEffect;
    }
    // Ordinary verification / build / lint — no EditPolicy approval required.
    if crate::governance::verification_family_for_command(command).is_some() {
        return ToolEffect::ReadOnly;
    }
    ToolEffect::ExternalSideEffect
}

fn is_package_install_shell(command: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    lower.contains("npm install")
        || lower.contains("npm i ")
        || lower.contains("npm i\t")
        || lower.contains("pnpm install")
        || lower.contains("pnpm add")
        || lower.contains("yarn add")
        || lower.contains("yarn install")
        || lower.contains("pip install")
        || lower.contains("pip3 install")
        || lower.contains("cargo install")
        || lower.contains("apt install")
        || lower.contains("brew install")
}

pub fn build_approval_request(action: &Action, effect: ToolEffect) -> ApprovalRequest {
    let (summary, proposed_scope, preview) = match action {
        Action::WriteFile { path, content } => (
            format!("write_file {}", path.display()),
            Scope::Paths(vec![path.clone()]),
            Some(truncate_preview(&format_write_preview(content), 8_000)),
        ),
        Action::ApplyPatch { path, diff } => (
            format!("apply_patch {}", path.display()),
            Scope::Paths(vec![path.clone()]),
            Some(truncate_preview(diff, 12_000)),
        ),
        Action::RunCommand { cmd, cwd, .. } => (
            format!("run_command in {}: {cmd}", cwd.display()),
            Scope::Unspecified,
            None,
        ),
        Action::RunShell { command, cwd, .. } => (
            format!("run_shell in {}: {command}", cwd.display()),
            Scope::Unspecified,
            None,
        ),
        other => (format!("{other:?}"), Scope::Unspecified, None),
    };

    let reason = match effect {
        ToolEffect::WorkspaceMutation => {
            "Edit policy requires approval before workspace mutations".to_string()
        }
        ToolEffect::DestructiveMutation => {
            "Destructive mutations require separate approval".to_string()
        }
        ToolEffect::ExternalSideEffect => {
            "External side effects require separate approval (not covered by ApproveForTask)"
                .to_string()
        }
        ToolEffect::ReadOnly => "Unexpected approval request for read-only tool".to_string(),
    };

    ApprovalRequest {
        id: ApprovalId::new(),
        effect,
        summary,
        proposed_scope,
        reason,
        expires_at_revision: None,
        preview,
    }
}

fn format_write_preview(content: &str) -> String {
    let mut out = String::from("--- /dev/null\n+++ proposed write\n");
    for line in content.lines() {
        out.push('+');
        out.push_str(line);
        out.push('\n');
    }
    if !content.ends_with('\n') && !content.is_empty() {
        // already newline-terminated per line loop for content.lines(); ok
    }
    out
}

fn truncate_preview(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max_chars).collect();
    out.push_str("\n… [preview truncated]");
    out
}

/// Decide whether `effect` may proceed under the current policy / grants.
pub fn authorize(effect: ToolEffect, auth: &AuthorizationState) -> AuthorizationDecision {
    match auth.edit_policy {
        EditPolicy::Auto => AuthorizationDecision::Allow,
        EditPolicy::ReadOnly => match effect {
            ToolEffect::ReadOnly => AuthorizationDecision::Allow,
            _ => AuthorizationDecision::Deny(
                "edit policy is ReadOnly — mutating / external tools are denied".to_string(),
            ),
        },
        EditPolicy::ConfirmBeforeFirstEdit => match effect {
            ToolEffect::ReadOnly => AuthorizationDecision::Allow,
            ToolEffect::WorkspaceMutation => {
                if auth.workspace_mutations_approved {
                    AuthorizationDecision::Allow
                } else {
                    // Caller fills request via build_approval_request.
                    AuthorizationDecision::RequireApproval(ApprovalRequest {
                        id: ApprovalId::new(),
                        effect,
                        summary: String::new(),
                        proposed_scope: Scope::Unspecified,
                        reason: "ConfirmBeforeFirstEdit: workspace mutation needs approval"
                            .to_string(),
                        expires_at_revision: None,
                        preview: None,
                    })
                }
            }
            ToolEffect::DestructiveMutation | ToolEffect::ExternalSideEffect => {
                AuthorizationDecision::RequireApproval(ApprovalRequest {
                    id: ApprovalId::new(),
                    effect,
                    summary: String::new(),
                    proposed_scope: Scope::Unspecified,
                    reason: match effect {
                        ToolEffect::DestructiveMutation => {
                            "destructive mutation needs separate approval".to_string()
                        }
                        _ => "external side effect needs separate approval".to_string(),
                    },
                    expires_at_revision: None,
                    preview: None,
                })
            }
        },
    }
}

/// Apply a user decision. `ApproveForTask` only grants ordinary workspace mutations.
pub fn apply_decision(
    auth: &mut AuthorizationState,
    effect: ToolEffect,
    decision: &ApprovalDecision,
) {
    match decision {
        ApprovalDecision::Reject | ApprovalDecision::ApproveOnce => {}
        ApprovalDecision::ApproveForTask => {
            if matches!(effect, ToolEffect::WorkspaceMutation) {
                auth.workspace_mutations_approved = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn patch_action() -> Action {
        Action::ApplyPatch {
            path: PathBuf::from("src/a.ts"),
            diff: "--- a\n+++ b\n@@\n-x\n+y\n".into(),
        }
    }

    fn shell_action() -> Action {
        Action::RunShell {
            command: "pytest".into(),
            cwd: PathBuf::from("."),
            timeout_secs: 30,
        }
    }

    #[test]
    fn build_approval_includes_patch_preview() {
        let req = build_approval_request(&patch_action(), ToolEffect::WorkspaceMutation);
        assert!(req.summary.contains("apply_patch"));
        let preview = req.preview.expect("preview");
        assert!(preview.contains("-x"));
        assert!(preview.contains("+y"));
    }

    #[test]
    fn tool_effect_classifies_common_actions() {
        assert_eq!(
            tool_effect(&Action::GitStatus),
            ToolEffect::ReadOnly
        );
        assert_eq!(tool_effect(&patch_action()), ToolEffect::WorkspaceMutation);
        // Ordinary verify shells are read-only for approval (ApproveForTask UX).
        assert_eq!(tool_effect(&shell_action()), ToolEffect::ReadOnly);
        assert_eq!(
            tool_effect(&Action::RunShell {
                command: "npm run lint".into(),
                cwd: PathBuf::from("frontend-v2"),
                timeout_secs: 60,
            }),
            ToolEffect::ReadOnly
        );
        assert_eq!(
            tool_effect(&Action::RunShell {
                command: "npm install lodash".into(),
                cwd: PathBuf::from("."),
                timeout_secs: 60,
            }),
            ToolEffect::ExternalSideEffect
        );
        assert_eq!(
            tool_effect(&Action::RunShell {
                command: "cat -n frontend-v2/src/lib/api.ts".into(),
                cwd: PathBuf::from("."),
                timeout_secs: 30,
            }),
            ToolEffect::ReadOnly
        );
        assert_eq!(
            tool_effect(&Action::RunShell {
                command: "cat frontend-v2/src/lib/api.ts | head -120".into(),
                cwd: PathBuf::from("."),
                timeout_secs: 30,
            }),
            ToolEffect::ReadOnly
        );
    }

    #[test]
    fn confirm_allows_explore_shell_without_approval() {
        let auth = AuthorizationState::new(EditPolicy::ConfirmBeforeFirstEdit);
        assert!(matches!(
            authorize(ToolEffect::ReadOnly, &auth),
            AuthorizationDecision::Allow
        ));
        // Package installs still require approval.
        assert!(matches!(
            authorize(ToolEffect::ExternalSideEffect, &auth),
            AuthorizationDecision::RequireApproval(_)
        ));
    }

    #[test]
    fn auto_allows_mutations() {
        let auth = AuthorizationState::new(EditPolicy::Auto);
        assert!(matches!(
            authorize(ToolEffect::WorkspaceMutation, &auth),
            AuthorizationDecision::Allow
        ));
    }

    #[test]
    fn readonly_denies_mutations() {
        let auth = AuthorizationState::new(EditPolicy::ReadOnly);
        match authorize(ToolEffect::WorkspaceMutation, &auth) {
            AuthorizationDecision::Deny(msg) => assert!(msg.contains("ReadOnly")),
            other => panic!("expected Deny, got {other:?}"),
        }
    }

    #[test]
    fn confirm_requires_then_allows_after_task_grant() {
        let mut auth = AuthorizationState::new(EditPolicy::ConfirmBeforeFirstEdit);
        assert!(matches!(
            authorize(ToolEffect::WorkspaceMutation, &auth),
            AuthorizationDecision::RequireApproval(_)
        ));

        apply_decision(
            &mut auth,
            ToolEffect::WorkspaceMutation,
            &ApprovalDecision::ApproveForTask,
        );
        assert!(auth.workspace_mutations_approved);
        assert!(matches!(
            authorize(ToolEffect::WorkspaceMutation, &auth),
            AuthorizationDecision::Allow
        ));

        // External still requires approval — ApproveForTask does not cover it.
        assert!(matches!(
            authorize(ToolEffect::ExternalSideEffect, &auth),
            AuthorizationDecision::RequireApproval(_)
        ));
    }

    #[test]
    fn approve_for_task_ignored_for_external() {
        let mut auth = AuthorizationState::new(EditPolicy::ConfirmBeforeFirstEdit);
        apply_decision(
            &mut auth,
            ToolEffect::ExternalSideEffect,
            &ApprovalDecision::ApproveForTask,
        );
        assert!(!auth.workspace_mutations_approved);
    }
}
