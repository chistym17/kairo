use std::collections::{HashMap, HashSet};

use super::fingerprint::VerificationFamily;
use super::pre_edit::PreEditTracker;
use super::{POST_VERIFY_FINISH_TURNS, STAGNATION_TURNS_AFTER_EDIT};

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
    /// Enough evidence to edit; broad exploration restricted.
    EditOrJustify,
    ReviewOrFinish,
}

#[derive(Debug, Clone, Default)]
pub struct GovernanceMetrics {
    pub governance_rejections: u32,
    pub repeated_actions_blocked: u32,
    pub verification_families_blocked: u32,
    pub review_or_finish_transitions: u32,
    pub no_progress_terminations: u32,
    pub edit_or_justify_transitions: u32,
}

#[derive(Debug)]
pub struct GovernanceState {
    /// When false (default), semantic mode gating / rejections are disabled.
    /// Code remains for comparison; see phasev2.md Phase A.
    pub semantic_policy: bool,

    pub workspace_revision: u64,

    pub last_edit_turn: Option<u32>,
    pub last_successful_verify_turn: Option<u32>,
    pub last_unique_info_turn: Option<u32>,

    pub turns_since_edit: u32,
    pub turns_since_unique_info: u32,
    pub turns_since_successful_verify: u32,

    pub consecutive_blocked_actions: u32,
    pub repeated_action_counts: HashMap<u64, u32>,
    pub verify_family_failures: HashMap<VerificationFamily, u32>,
    pub blocked_verification_families: HashSet<VerificationFamily>,

    pub escalation: EscalationLevel,
    pub mode: RuntimeMode,

    pub review_mode_violations: u32,
    pub has_successful_edit: bool,
    pub files_ever_modified: HashSet<String>,

    pub pre_edit: PreEditTracker,
    pub consecutive_no_tool_turns: u32,
    pub no_edit_escalation_level: u32,

    pub metrics: GovernanceMetrics,
    pub pending_guidance: Option<String>,
}

impl Default for GovernanceState {
    fn default() -> Self {
        Self {
            semantic_policy: false,
            workspace_revision: 0,
            last_edit_turn: None,
            last_successful_verify_turn: None,
            last_unique_info_turn: None,
            turns_since_edit: 0,
            turns_since_unique_info: 0,
            turns_since_successful_verify: 0,
            consecutive_blocked_actions: 0,
            repeated_action_counts: HashMap::new(),
            verify_family_failures: HashMap::new(),
            blocked_verification_families: HashSet::new(),
            escalation: EscalationLevel::None,
            mode: RuntimeMode::Normal,
            review_mode_violations: 0,
            has_successful_edit: false,
            files_ever_modified: HashSet::new(),
            pre_edit: PreEditTracker::new(),
            consecutive_no_tool_turns: 0,
            no_edit_escalation_level: 0,
            metrics: GovernanceMetrics::default(),
            pending_guidance: None,
        }
    }
}

impl GovernanceState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_semantic_policy(mut self, enabled: bool) -> Self {
        self.semantic_policy = enabled;
        self
    }

    pub fn tool_policy(&self) -> super::policy::ToolPolicy {
        if !self.semantic_policy {
            return super::policy::ToolPolicy::unrestricted();
        }
        super::policy::ToolPolicy::from_state(self)
    }

    pub fn take_pending_guidance(&mut self) -> Option<String> {
        self.pending_guidance.take()
    }

    pub fn note_file_modified(&mut self, path: &str) {
        self.files_ever_modified.insert(path.replace('\\', "/"));
    }

    pub fn note_edit(&mut self, model_turn: u32) {
        self.workspace_revision = self.workspace_revision.saturating_add(1);
        self.last_edit_turn = Some(model_turn);
        self.turns_since_edit = 0;
        self.turns_since_unique_info = 0;
        self.turns_since_successful_verify = 0;
        self.consecutive_blocked_actions = 0;
        self.repeated_action_counts.clear();
        self.verify_family_failures.clear();
        self.blocked_verification_families.clear();
        self.review_mode_violations = 0;
        self.has_successful_edit = true;
        self.escalation = EscalationLevel::None;
        self.mode = RuntimeMode::Normal;
        self.pre_edit.note_edit();
        self.consecutive_no_tool_turns = 0;
        self.no_edit_escalation_level = 0;
    }

    pub fn note_unique_info(&mut self, model_turn: u32) {
        self.last_unique_info_turn = Some(model_turn);
        self.turns_since_unique_info = 0;
        self.consecutive_blocked_actions = 0;
    }

    pub fn note_successful_verify(&mut self, model_turn: u32) {
        self.last_successful_verify_turn = Some(model_turn);
        self.turns_since_successful_verify = 0;
        self.consecutive_blocked_actions = 0;
    }

    pub fn note_no_tool_turn(&mut self) {
        self.consecutive_no_tool_turns = self.consecutive_no_tool_turns.saturating_add(1);
    }

    pub fn note_tool_turn(&mut self) {
        self.consecutive_no_tool_turns = 0;
    }

    pub fn set_no_edit_escalation_level(&mut self, level: u32) {
        self.no_edit_escalation_level = level;
        if !self.semantic_policy {
            return;
        }
        if level >= 4 && self.mode == RuntimeMode::Normal {
            self.enter_edit_or_justify(
                "no-edit budget exhausted — patch, one justified new read, update plan, or finish.",
            );
        } else if level == 3 {
            self.pending_guidance = Some(
                "Hard warning: you have explored long enough without editing. \
                 Your next response must apply_patch/write_file, cite one specific missing fact, \
                 or finish with a blocker."
                    .to_string(),
            );
        }
    }

    pub fn record_blocked_fingerprint(&mut self, fp_hash: u64) {
        self.consecutive_blocked_actions = self.consecutive_blocked_actions.saturating_add(1);
        *self.repeated_action_counts.entry(fp_hash).or_insert(0) += 1;
        self.metrics.repeated_actions_blocked += 1;
    }

    pub fn blocked_count_for(&self, fp_hash: u64) -> u32 {
        self.repeated_action_counts.get(&fp_hash).copied().unwrap_or(0)
    }

    pub fn enter_edit_or_justify(&mut self, reason: &str) {
        if !self.semantic_policy {
            return;
        }
        if self.mode == RuntimeMode::Normal {
            self.metrics.edit_or_justify_transitions += 1;
        }
        self.mode = RuntimeMode::EditOrJustify;
        self.escalation = EscalationLevel::Restrict;
        self.pending_guidance = Some(format!(
            "Runtime governance: {reason} Allowed next actions: apply_patch/write_file, \
             update_plan, one targeted read_file of a file/range not yet in context, \
             git_diff/git_status, or finish with an explicit blocker."
        ));
    }

    pub fn enter_review_or_finish(&mut self, reason: &str) {
        if !self.semantic_policy {
            return;
        }
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
        if !self.semantic_policy {
            return;
        }
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

    pub fn end_turn(&mut self, model_turn: u32, edited_this_turn: bool) {
        if self.last_edit_turn.is_some() && !edited_this_turn {
            self.turns_since_edit = self.turns_since_edit.saturating_add(1);
        }
        if self.last_unique_info_turn.is_some() {
            self.turns_since_unique_info = self.turns_since_unique_info.saturating_add(1);
        }
        if self.last_successful_verify_turn.is_some() {
            self.turns_since_successful_verify =
                self.turns_since_successful_verify.saturating_add(1);
        }

        if !self.semantic_policy {
            let _ = model_turn;
            return;
        }

        self.pre_edit.end_turn(model_turn, edited_this_turn);

        if self.pre_edit.should_enter_edit_or_justify() && self.mode == RuntimeMode::Normal {
            self.enter_edit_or_justify(
                "enough evidence exists to edit — pre-edit stagnation window expired.",
            );
        }

        let edited = self.last_edit_turn.is_some();
        let verified_after_edit = match (self.last_edit_turn, self.last_successful_verify_turn) {
            (Some(e), Some(v)) => v >= e,
            _ => false,
        };

        // Post-verify finish gating: verify passed, no new edits, short tail allowed.
        if verified_after_edit
            && self.mode == RuntimeMode::Normal
            && self.turns_since_successful_verify >= POST_VERIFY_FINISH_TURNS
            && self.turns_since_edit >= POST_VERIFY_FINISH_TURNS
        {
            self.enter_review_or_finish(
                "successful verification with no further edits — finish or make a corrective edit.",
            );
            return;
        }

        let stagnant = edited
            && !verified_after_edit
            && self.turns_since_edit >= STAGNATION_TURNS_AFTER_EDIT
            && self.turns_since_unique_info >= STAGNATION_TURNS_AFTER_EDIT;

        if stagnant && self.mode == RuntimeMode::Normal {
            self.enter_review_or_finish(
                "no progress after edit — no successful verification and no new information.",
            );
        }
        let _ = model_turn;
    }
}
