#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::governance::{
        fingerprint, fingerprint_hash, ActionProgress, GovernanceDecision, GovernanceState,
        RuntimeMode, BLOCKED_REPEAT_THRESHOLD,
    };
    use crate::state::step::Action;
    use crate::tools::ToolResult;

    #[test]
    fn blocks_repeated_grep_after_threshold() {
        let mut gov = GovernanceState::new();
        gov.note_edit(1);
        let action = Action::RunCommand {
            cmd: "grep position frontend-v2/src/lib/api.ts".to_string(),
            cwd: PathBuf::from("."),
            timeout_secs: 30,
        };
        let blocked = ToolResult {
            success: false,
            output: String::new(),
            diff: None,
            error: Some("Blocked: explore-shell budget exhausted".to_string()),
            stop_reason: None,
        };

        for _ in 0..BLOCKED_REPEAT_THRESHOLD {
            assert!(matches!(
                gov.before_action(&action, 2),
                GovernanceDecision::Allow
            ));
            gov.after_action(
                &action,
                &blocked,
                ActionProgress::NoProgress,
                2,
            );
        }

        assert!(matches!(
            gov.before_action(&action, 3),
            GovernanceDecision::Reject { .. }
        ));
    }

    #[test]
    fn review_or_finish_blocks_explore_reads() {
        let mut gov = GovernanceState::new();
        gov.enter_review_or_finish("test");
        assert_eq!(gov.mode, RuntimeMode::ReviewOrFinish);

        let read = Action::ReadFile {
            path: PathBuf::from("src/lib/api.ts"),
            offset: 1,
            limit: 50,
        };
        assert!(matches!(
            gov.before_action(&read, 1),
            GovernanceDecision::Reject { .. }
        ));

        let finish = Action::Finish {
            summary: "done".to_string(),
        };
        assert!(matches!(
            gov.before_action(&finish, 1),
            GovernanceDecision::Allow
        ));
    }

    #[test]
    fn edit_resets_blocked_fingerprints() {
        let mut gov = GovernanceState::new();
        gov.note_edit(1);
        let action = Action::RunCommand {
            cmd: "grep foo".to_string(),
            cwd: PathBuf::from("."),
            timeout_secs: 30,
        };
        let blocked = ToolResult {
            success: false,
            output: String::new(),
            diff: None,
            error: Some("Blocked: explore-shell budget".to_string()),
            stop_reason: None,
        };
        for _ in 0..BLOCKED_REPEAT_THRESHOLD {
            gov.after_action(&action, &blocked, ActionProgress::NoProgress, 2);
        }
        assert!(gov.blocked_count_for(fingerprint_hash(&fingerprint(
            &action, gov.workspace_revision
        ))) >= BLOCKED_REPEAT_THRESHOLD);

        gov.note_edit(5);
        assert!(matches!(
            gov.before_action(&action, 6),
            GovernanceDecision::Allow
        ));
    }
}
