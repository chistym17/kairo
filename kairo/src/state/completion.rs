//! Objective post-edit completion — no semantic readiness inference.
//!
//! After the model has edited, reviewed workspace state, and attempted
//! verification, idle turns with no further progress end the run as
//! `CompletedUnverified`.
//!
//! Once those milestones are met, only a new edit or a *successful* verify
//! resets the idle clock. Extra reads/searches must not postpone completion.

use std::collections::HashSet;

use crate::verification::VerificationOutcome;

/// Idle model turns after edit+review+verify-attempt before auto-stop.
pub const POST_EDIT_IDLE_TURNS: u32 = 4;

#[derive(Debug, Default, Clone)]
pub struct CompletionTracker {
    pub last_edit_turn: Option<u32>,
    pub reviewed_since_edit: bool,
    pub verify_attempted_since_edit: bool,
    /// Turn when edit + review + verify-attempt were all satisfied.
    pub milestones_met_turn: Option<u32>,
    /// Last turn that produced completion-relevant progress.
    pub last_progress_turn: Option<u32>,
    seen_result_fingerprints: HashSet<u64>,
    pub last_verify_success: Option<bool>,
    pub last_verify_detail: Option<String>,
    pub last_verification: Option<VerificationOutcome>,
}

impl CompletionTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn note_edit(&mut self, turn: u32) {
        self.last_edit_turn = Some(turn);
        self.reviewed_since_edit = false;
        self.verify_attempted_since_edit = false;
        self.milestones_met_turn = None;
        self.last_progress_turn = Some(turn);
        self.seen_result_fingerprints.clear();
        self.last_verify_success = None;
        self.last_verify_detail = None;
        self.last_verification = None;
    }

    /// Record a novel tool result. Before milestones, this can move the progress
    /// cursor. After milestones, thrash reads/searches are ignored so they cannot
    /// postpone auto-completion.
    pub fn note_novel_result(&mut self, turn: u32, fingerprint: u64) -> bool {
        if self.last_edit_turn.is_none() {
            return false;
        }
        if !self.seen_result_fingerprints.insert(fingerprint) {
            return false;
        }
        // Milestones already met: only edit / successful verify reset idle.
        if self.milestones_met_turn.is_some() {
            return false;
        }
        self.last_progress_turn = Some(turn);
        true
    }

    pub fn note_review(&mut self, turn: u32) {
        if self.last_edit_turn.is_none() {
            return;
        }
        self.reviewed_since_edit = true;
        self.maybe_mark_milestones(turn);
    }

    /// Record an executed command that matched a discovered repository check.
    pub fn note_verification_outcome(
        &mut self,
        turn: u32,
        outcome: VerificationOutcome,
    ) {
        if self.last_edit_turn.is_none() {
            return;
        }
        self.verify_attempted_since_edit = true;
        self.last_verify_success = Some(outcome.passed());
        self.last_verify_detail = Some(outcome.summary.clone());
        if outcome.passed() {
            self.last_progress_turn = Some(turn);
        }
        self.last_verification = Some(outcome);
        self.maybe_mark_milestones(turn);
    }

    fn maybe_mark_milestones(&mut self, turn: u32) {
        if self.milestones_met_turn.is_some() {
            return;
        }
        if self.last_edit_turn.is_some()
            && self.reviewed_since_edit
            && self.verify_attempted_since_edit
        {
            self.milestones_met_turn = Some(turn);
        }
    }

    /// Idle turns since the later of milestones-met and last progress.
    pub fn idle_turns(&self, current_turn: u32) -> Option<u32> {
        let m = self.milestones_met_turn?;
        let last = self.last_progress_turn.unwrap_or(m).max(m);
        Some(current_turn.saturating_sub(last))
    }

    pub fn should_complete_unverified(&self, current_turn: u32) -> bool {
        match self.idle_turns(current_turn) {
            Some(idle) => idle >= POST_EDIT_IDLE_TURNS,
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verification::{CheckKind, VerificationOutcome, VerificationStatus};

    fn outcome(passed: bool, summary: &str) -> VerificationOutcome {
        VerificationOutcome {
            check_id: "root:test".into(),
            kind: CheckKind::Test,
            status: if passed {
                VerificationStatus::Passed
            } else {
                VerificationStatus::Failed
            },
            exit_code: Some(if passed { 0 } else { 1 }),
            summary: summary.into(),
        }
    }

    fn env_blocked(summary: &str) -> VerificationOutcome {
        VerificationOutcome {
            check_id: "root:test".into(),
            kind: CheckKind::Test,
            status: VerificationStatus::EnvironmentBlocked,
            exit_code: Some(133),
            summary: summary.into(),
        }
    }

    #[test]
    fn completes_after_edit_review_verify_and_idle() {
        let mut t = CompletionTracker::new();
        t.note_edit(10);
        assert!(!t.should_complete_unverified(14));
        t.note_review(11);
        assert!(!t.should_complete_unverified(14));
        t.note_verification_outcome(12, outcome(false, "exited with code 1"));
        assert_eq!(t.milestones_met_turn, Some(12));
        assert!(!t.should_complete_unverified(15)); // idle 3 from max(10,12)=12
        assert!(t.should_complete_unverified(16)); // idle 4
    }

    #[test]
    fn successful_verify_resets_idle() {
        let mut t = CompletionTracker::new();
        t.note_edit(1);
        t.note_review(2);
        t.note_verification_outcome(3, outcome(true, "3 passed"));
        assert!(!t.should_complete_unverified(6));
        assert!(t.should_complete_unverified(7));
    }

    #[test]
    fn thrash_reads_do_not_reset_idle_after_milestones() {
        let mut t = CompletionTracker::new();
        t.note_edit(1);
        t.note_review(2);
        t.note_verification_outcome(3, outcome(false, "failed"));
        assert_eq!(t.milestones_met_turn, Some(3));

        // Distinct fingerprints after milestones must not postpone auto-complete.
        assert!(!t.note_novel_result(5, 42));
        assert!(!t.note_novel_result(6, 43));
        assert!(!t.should_complete_unverified(6)); // idle 3
        assert!(t.should_complete_unverified(7)); // idle 4
    }

    #[test]
    fn successful_verify_after_failed_attempt_resets_idle() {
        let mut t = CompletionTracker::new();
        t.note_edit(1);
        t.note_review(2);
        t.note_verification_outcome(3, outcome(false, "failed"));
        assert!(t.should_complete_unverified(7));

        t.note_verification_outcome(8, outcome(true, "ok"));
        assert!(!t.should_complete_unverified(11));
        assert!(t.should_complete_unverified(12));
    }

    #[test]
    fn novel_results_before_milestones_still_track_progress() {
        let mut t = CompletionTracker::new();
        t.note_edit(1);
        assert!(t.note_novel_result(2, 99));
        assert_eq!(t.last_progress_turn, Some(2));
        assert!(!t.note_novel_result(3, 99));
        t.note_review(4);
        t.note_verification_outcome(5, outcome(false, "failed"));
        // Idle anchors at max(progress=2, milestones=5) = 5
        assert!(!t.should_complete_unverified(8));
        assert!(t.should_complete_unverified(9));
    }

    #[test]
    fn environment_blocked_counts_as_verify_attempt_without_resetting_idle() {
        let mut t = CompletionTracker::new();
        t.note_edit(1);
        t.note_review(2);
        t.note_verification_outcome(
            3,
            env_blocked("EnvironmentBlocked: sandbox crash"),
        );
        assert!(t.verify_attempted_since_edit);
        assert_eq!(t.milestones_met_turn, Some(3));
        assert!(!t.last_verify_success.unwrap_or(true));
        assert!(t.should_complete_unverified(7));
    }
}
