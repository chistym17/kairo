use crate::state::step::Action;
use crate::state::stop_reason::StopReason;
use crate::tools::ToolResult;

use super::fingerprint::{
    fingerprint, fingerprint_hash, ActionFingerprint,
};
use super::policy::tool_name;
use super::state::{GovernanceState, RuntimeMode};
use super::{
    BLOCKED_REPEAT_THRESHOLD, REVIEW_MODE_VIOLATIONS_BEFORE_TERMINATE, VERIFY_FAMILY_FAILURE_THRESHOLD,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GovernanceBlockCode {
    RepeatedBlockedAction,
    RepeatedRead,
    ToolNotAllowedInMode,
    VerificationFamilyExhausted,
    ReviewModeViolation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GovernanceDecision {
    Allow,
    Reject {
        code: GovernanceBlockCode,
        message: String,
    },
    Terminate(StopReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionProgress {
    UniqueInfo,
    Edit,
    SuccessfulVerify,
    ReviewOnly,
    NoProgress,
    Failed,
}

impl GovernanceState {
    pub fn before_action(&mut self, action: &Action, _model_turn: u32) -> GovernanceDecision {
        if !self.semantic_policy {
            return GovernanceDecision::Allow;
        }

        let policy = self.tool_policy();
        let fp = fingerprint(action, self.workspace_revision);
        let fp_hash = fingerprint_hash(&fp);

        if matches!(action, Action::Finish { .. }) {
            return GovernanceDecision::Allow;
        }

        if !policy.allows_action(action) {
            self.review_mode_violations = self.review_mode_violations.saturating_add(1);
            self.metrics.governance_rejections += 1;

            if self.mode == RuntimeMode::ReviewOrFinish
                && self.review_mode_violations >= REVIEW_MODE_VIOLATIONS_BEFORE_TERMINATE
            {
                self.metrics.no_progress_terminations += 1;
                return GovernanceDecision::Terminate(terminate_reason(self));
            }

            return GovernanceDecision::Reject {
                code: GovernanceBlockCode::ToolNotAllowedInMode,
                message: format!(
                    "Governance: {:?} is not available in the current runtime mode ({:?}). \
                     Allowed tools are restricted for this phase.",
                    tool_name(action),
                    self.mode
                ),
            };
        }

        if let ActionFingerprint::Verify { family, .. } = &fp {
            if self.blocked_verification_families.contains(family) {
                self.metrics.governance_rejections += 1;
                return GovernanceDecision::Reject {
                    code: GovernanceBlockCode::VerificationFamilyExhausted,
                    message: format!(
                        "Governance: verification family {:?} already failed {} times since the \
                         last edit. Edit the code, try a different check, or finish and explain \
                         that verification is blocked.",
                        family, VERIFY_FAMILY_FAILURE_THRESHOLD
                    ),
                };
            }
        }

        let prior_blocks = self.blocked_count_for(fp_hash);
        if prior_blocks >= BLOCKED_REPEAT_THRESHOLD {
            self.escalate_from_repeats(prior_blocks);
            self.metrics.governance_rejections += 1;
            return GovernanceDecision::Reject {
                code: GovernanceBlockCode::RepeatedBlockedAction,
                message: format!(
                    "Governance: this action was already blocked {} times with no workspace \
                     change. Do not retry it. Allowed next actions: edit, review git diff/status, \
                     run a different verification, or finish honestly.",
                    prior_blocks
                ),
            };
        }

        if prior_blocks == BLOCKED_REPEAT_THRESHOLD - 1 {
            self.escalate_from_repeats(prior_blocks + 1);
        }

        GovernanceDecision::Allow
    }

    pub fn after_action(
        &mut self,
        action: &Action,
        result: &ToolResult,
        progress: ActionProgress,
        model_turn: u32,
    ) {
        if !self.semantic_policy {
            return;
        }

        let fp = fingerprint(action, self.workspace_revision);
        let fp_hash = fingerprint_hash(&fp);

        match progress {
            ActionProgress::Edit => {}
            ActionProgress::UniqueInfo => self.note_unique_info(model_turn),
            ActionProgress::SuccessfulVerify => self.note_successful_verify(model_turn),
            ActionProgress::ReviewOnly | ActionProgress::NoProgress | ActionProgress::Failed => {}
        }

        if is_blocked_result(result) {
            self.record_blocked_fingerprint(fp_hash);
            self.escalate_from_repeats(self.blocked_count_for(fp_hash));
        } else if result.success {
            self.consecutive_blocked_actions = 0;
        }

        if let ActionFingerprint::Verify { family, .. } = fp {
            if !result.success || is_blocked_result(result) {
                let count = self
                    .verify_family_failures
                    .entry(family)
                    .or_insert(0);
                *count += 1;
                if *count >= VERIFY_FAMILY_FAILURE_THRESHOLD {
                    if self.blocked_verification_families.insert(family) {
                        self.metrics.verification_families_blocked += 1;
                    }
                }
            } else {
                self.verify_family_failures.remove(&family);
            }
        }
    }
}

fn is_blocked_result(result: &ToolResult) -> bool {
    if !result.success {
        if let Some(err) = &result.error {
            let e = err.to_ascii_lowercase();
            if e.contains("governance:")
                || e.contains("explore-shell budget")
                || e.contains("blocked: identical")
                || e.contains("blocked: explore")
                || e.contains("repeated read blocked")
            {
                return true;
            }
        }
    }
    false
}

pub fn classify_progress(
    action: &Action,
    result: &ToolResult,
    read_cache_hit: bool,
) -> ActionProgress {
    if read_cache_hit {
        return ActionProgress::NoProgress;
    }
    if is_blocked_result(result) {
        return ActionProgress::NoProgress;
    }
    if !result.success {
        return ActionProgress::Failed;
    }
    match action {
        Action::ApplyPatch { .. } | Action::WriteFile { .. } => ActionProgress::Edit,
        Action::ReadFile { .. }
        | Action::SearchCode { .. }
        | Action::Locate { .. }
        | Action::FindFiles { .. }
        | Action::ListFiles { .. }
        | Action::DetectProjectSnapshot { .. } => ActionProgress::UniqueInfo,
        Action::RunCommand { .. } | Action::RunShell { .. } => {
            if result.success && looks_like_verify_success(action, result) {
                ActionProgress::SuccessfulVerify
            } else {
                ActionProgress::UniqueInfo
            }
        }
        Action::GitStatus | Action::GitDiff { .. } => ActionProgress::ReviewOnly,
        Action::UpdatePlan { .. } => ActionProgress::ReviewOnly,
        Action::Finish { .. } => ActionProgress::ReviewOnly,
    }
}

fn looks_like_verify_success(action: &Action, result: &ToolResult) -> bool {
    use super::fingerprint::verification_family_for_command;
    let cmd = match action {
        Action::RunCommand { cmd, .. } | Action::RunShell { command: cmd, .. } => cmd,
        _ => return false,
    };
    verification_family_for_command(cmd).is_some() && result.output.contains("exit_code: 0")
}

fn terminate_reason(state: &GovernanceState) -> StopReason {
    if state.has_successful_edit {
        StopReason::CompletedUnverified
    } else {
        StopReason::NoProgress
    }
}
