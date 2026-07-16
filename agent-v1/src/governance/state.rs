use std::collections::{HashMap, HashSet};

use super::fingerprint::VerificationFamily;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum EscalationLevel {
    #[default]
    None,
    Warn,
    Restrict,
    ReviewOrFinish,
    Terminate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RuntimeMode {
    #[default]
    Normal,
    ReviewOrFinish,
}

#[derive(Debug, Clone, Default)]
pub struct GovernanceMetrics {
    pub governance_rejections: u32,
    pub repeated_actions_blocked: u32,
    pub verification_families_blocked: u32,
    pub review_or_finish_transitions: u32,
    pub no_progress_terminations: u32,
}

#[derive(Debug, Default)]
pub struct GovernanceState {
    pub workspace_revision: u64,

    pub last_edit_turn: Option<u32>,
    pub last_successful_verify_turn: Option<u32>,
    pub last_unique_info_turn: Option<u32>,

    pub turns_since_edit: u32,
    pub turns_since_unique_info: u32,

    pub consecutive_blocked_actions: u32,
    pub repeated_action_counts: HashMap<u64, u32>,
    pub verify_family_failures: HashMap<VerificationFamily, u32>,
    pub blocked_verification_families: HashSet<VerificationFamily>,

    pub escalation: EscalationLevel,
    pub mode: RuntimeMode,

    pub review_mode_violations: u32,
    pub has_successful_edit: bool,

    pub metrics: GovernanceMetrics,
    pub pending_guidance: Option<String>,
}

impl GovernanceState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn tool_policy(&self) -> super::policy::ToolPolicy {
        super::policy::ToolPolicy::from_state(self)
    }

    pub fn take_pending_guidance(&mut self) -> Option<String> {
        self.pending_guidance.take()
    }

    pub fn note_edit(&mut self, model_turn: u32) {
        self.workspace_revision = self.workspace_revision.saturating_add(1);
        self.last_edit_turn = Some(model_turn);
        self.turns_since_edit = 0;
        self.turns_since_unique_info = 0;
        self.consecutive_blocked_actions = 0;
        self.repeated_action_counts.clear();
        self.verify_family_failures.clear();
        self.blocked_verification_families.clear();
        self.review_mode_violations = 0;
        self.has_successful_edit = true;
        self.escalation = EscalationLevel::None;
        self.mode = RuntimeMode::Normal;
    }

    pub fn note_unique_info(&mut self, model_turn: u32) {
        self.last_unique_info_turn = Some(model_turn);
        self.turns_since_unique_info = 0;
        self.consecutive_blocked_actions = 0;
    }

    pub fn note_successful_verify(&mut self, model_turn: u32) {
        self.last_successful_verify_turn = Some(model_turn);
        self.consecutive_blocked_actions = 0;
    }

    pub fn record_blocked_fingerprint(&mut self, fp_hash: u64) {
        self.consecutive_blocked_actions = self.consecutive_blocked_actions.saturating_add(1);
        *self.repeated_action_counts.entry(fp_hash).or_insert(0) += 1;
        self.metrics.repeated_actions_blocked += 1;
    }

    pub fn blocked_count_for(&self, fp_hash: u64) -> u32 {
        self.repeated_action_counts.get(&fp_hash).copied().unwrap_or(0)
    }

    pub fn enter_review_or_finish(&mut self, reason: &str) {
        if self.mode != RuntimeMode::ReviewOrFinish {
            self.metrics.review_or_finish_transitions += 1;
        }
        self.mode = RuntimeMode::ReviewOrFinish;
        self.escalation = EscalationLevel::ReviewOrFinish;
        self.pending_guidance = Some(format!(
            "Runtime governance: {reason} Allowed next actions: edit the repository, \
             review git diff/status, run one different relevant verification, or finish \
             honestly with what changed and what could not be verified."
        ));
    }

    pub fn escalate_from_repeats(&mut self, blocked_count: u32) {
        self.escalation = match blocked_count {
            0..=1 => EscalationLevel::None,
            2 => EscalationLevel::Warn,
            3 => EscalationLevel::Restrict,
            _ => EscalationLevel::ReviewOrFinish,
        };
        if self.escalation >= EscalationLevel::ReviewOrFinish {
            self.enter_review_or_finish(
                "repeated blocked actions with no new information.",
            );
        } else if self.escalation == EscalationLevel::Warn {
            self.pending_guidance = Some(
                "This action repeated a previous blocked attempt and produced no new information. \
                 Choose another strategy."
                    .to_string(),
            );
        }
    }

    pub fn end_turn(&mut self, model_turn: u32) {
        if self.last_edit_turn.is_some() {
            self.turns_since_edit = self.turns_since_edit.saturating_add(1);
        }
        if self.last_unique_info_turn.is_some() {
            self.turns_since_unique_info = self.turns_since_unique_info.saturating_add(1);
        }

        let edited = self.last_edit_turn.is_some();
        let verified_after_edit = match (self.last_edit_turn, self.last_successful_verify_turn) {
            (Some(e), Some(v)) => v >= e,
            _ => false,
        };
        let stagnant = edited
            && !verified_after_edit
            && self.turns_since_edit >= super::STAGNATION_TURNS_AFTER_EDIT
            && self.turns_since_unique_info >= super::STAGNATION_TURNS_AFTER_EDIT;

        if stagnant && self.mode == RuntimeMode::Normal {
            self.enter_review_or_finish(
                "no progress after edit — no successful verification and no new information.",
            );
        }
        let _ = model_turn;
    }
}
