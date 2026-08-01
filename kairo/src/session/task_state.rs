//! Narrow durable task memory for an interactive session (Phase 12).
//!
//! Intentionally small: objective, plan, decisions, modified files, verification.
//! `repository_notes` is deferred until the approval loop is proven.

use std::path::PathBuf;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::state::AgentPlan;
use crate::verification::{CheckKind, VerificationOutcome, VerificationStatus};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct TaskSessionState {
    pub objective: Option<String>,
    pub plan: Option<AgentPlan>,
    pub decisions: Vec<Decision>,
    pub modified_files: Vec<PathBuf>,
    pub verification: Vec<VerificationSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Decision {
    pub id: String,
    pub text: String,
    pub created_at: String,
    pub status: DecisionStatus,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum DecisionStatus {
    #[default]
    Active,
    Superseded,
}

/// Compact verification record for session persistence.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerificationSummary {
    pub check_id: String,
    pub kind: CheckKind,
    pub status: VerificationStatus,
    pub summary: String,
    pub recorded_at: String,
}

impl From<&VerificationOutcome> for VerificationSummary {
    fn from(outcome: &VerificationOutcome) -> Self {
        Self {
            check_id: outcome.check_id.clone(),
            kind: outcome.kind,
            status: outcome.status,
            summary: outcome.summary.clone(),
            recorded_at: Utc::now().to_rfc3339(),
        }
    }
}

impl TaskSessionState {
    pub fn set_objective_if_empty(&mut self, objective: impl Into<String>) {
        if self.objective.as_ref().map(|s| s.trim().is_empty()).unwrap_or(true) {
            let text = objective.into();
            let text = text.trim();
            if !text.is_empty() {
                self.objective = Some(text.to_string());
            }
        }
    }

    pub fn record_decision(&mut self, text: impl Into<String>) -> Result<&Decision, String> {
        let text = text.into();
        let text = text.trim();
        if text.is_empty() {
            return Err("decision text must not be empty".to_string());
        }
        self.decisions.push(Decision {
            id: Uuid::new_v4().to_string(),
            text: text.to_string(),
            created_at: Utc::now().to_rfc3339(),
            status: DecisionStatus::Active,
        });
        Ok(self.decisions.last().expect("just pushed"))
    }

    pub fn note_modified_file(&mut self, path: impl Into<PathBuf>) {
        let path = path.into();
        if !self.modified_files.iter().any(|p| p == &path) {
            self.modified_files.push(path);
        }
    }

    pub fn note_verification(&mut self, outcome: &VerificationOutcome) {
        self.verification.push(VerificationSummary::from(outcome));
        // Keep a bounded trail in the session.
        const MAX: usize = 32;
        if self.verification.len() > MAX {
            let skip = self.verification.len() - MAX;
            self.verification.drain(0..skip);
        }
    }

    pub fn render_compact(&self) -> String {
        let mut out = String::new();
        if let Some(obj) = &self.objective {
            out.push_str(&format!("Objective: {obj}\n"));
        } else {
            out.push_str("Objective: (none)\n");
        }
        match &self.plan {
            Some(plan) => {
                out.push_str(&format!(
                    "Plan: {} v{} [{:?}]\n",
                    plan.id.get(..8).unwrap_or(&plan.id),
                    plan.version,
                    plan.status
                ));
            }
            None => out.push_str("Plan: (none)\n"),
        }
        if !self.decisions.is_empty() {
            out.push_str("Decisions:\n");
            for d in self.decisions.iter().filter(|d| d.status == DecisionStatus::Active).rev().take(5) {
                out.push_str(&format!("  - {}\n", d.text));
            }
        }
        if !self.modified_files.is_empty() {
            out.push_str("Modified files:\n");
            for p in self.modified_files.iter().rev().take(12) {
                out.push_str(&format!("  - {}\n", p.display()));
            }
        }
        if let Some(last) = self.verification.last() {
            out.push_str(&format!(
                "Last verification: {} {:?} — {}\n",
                last.check_id, last.status, last.summary
            ));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verification::CheckKind;

    #[test]
    fn tracks_files_decisions_and_verification() {
        let mut task = TaskSessionState::default();
        task.set_objective_if_empty("Fix date filter");
        task.record_decision("frontend only").unwrap();
        task.note_modified_file("frontend/api.ts");
        task.note_modified_file("frontend/api.ts"); // dedupe
        assert_eq!(task.modified_files.len(), 1);

        let outcome = VerificationOutcome {
            check_id: "root:test".into(),
            kind: CheckKind::Test,
            status: VerificationStatus::Passed,
            exit_code: Some(0),
            summary: "ok".into(),
            command: "npm test".into(),
            scope: crate::verification::VerificationScope::ProjectUnit,
            affected_paths: vec![],
        };
        task.note_verification(&outcome);
        assert_eq!(task.verification.len(), 1);
        assert!(task.render_compact().contains("Fix date filter"));
        assert!(task.render_compact().contains("frontend only"));
    }
}
