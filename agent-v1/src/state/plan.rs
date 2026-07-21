use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Durable, approvable working plan (session artifact — not a required mode).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentPlan {
    /// Stable id across in-place amendments.
    #[serde(default = "new_plan_id")]
    pub id: String,
    /// Bumped on each amendment; no history store in v1.
    #[serde(default = "default_version")]
    pub version: u32,
    pub objective: String,
    pub steps: Vec<PlanStep>,
    #[serde(default)]
    pub status: PlanStatus,
    #[serde(default)]
    pub constraints: Vec<PlanConstraint>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanStep {
    pub text: String,
    pub done: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    #[default]
    Draft,
    Approved,
}

/// User or runtime constraint attached to the plan (e.g. "frontend only").
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanConstraint {
    pub text: String,
}

fn new_plan_id() -> String {
    Uuid::new_v4().to_string()
}

fn default_version() -> u32 {
    1
}

impl AgentPlan {
    pub const MAX_STEPS: usize = 20;

    /// Build a new plan (version 1, Draft) from an objective + newline checklist.
    /// Lines may use `[x]` / `[ ]` prefixes; bare lines are incomplete.
    pub fn from_update(objective: &str, steps_text: &str) -> Result<Self, String> {
        let (objective, steps) = parse_objective_and_steps(objective, steps_text)?;
        Ok(Self {
            id: new_plan_id(),
            version: 1,
            objective,
            steps,
            status: PlanStatus::Draft,
            constraints: Vec::new(),
        })
    }

    /// Replace objective/steps in place: keep `id` + constraints, bump `version`, set Draft.
    pub fn amend_from_update(&mut self, objective: &str, steps_text: &str) -> Result<(), String> {
        let (objective, steps) = parse_objective_and_steps(objective, steps_text)?;
        self.objective = objective;
        self.steps = steps;
        self.version = self.version.saturating_add(1);
        self.status = PlanStatus::Draft;
        Ok(())
    }

    pub fn approve(&mut self) {
        self.status = PlanStatus::Approved;
    }

    /// Add a constraint and return the plan to Draft (version bump).
    pub fn add_constraint(&mut self, text: impl Into<String>) -> Result<(), String> {
        let text = text.into();
        let text = text.trim();
        if text.is_empty() {
            return Err("constraint text must not be empty".to_string());
        }
        self.constraints.push(PlanConstraint {
            text: text.to_string(),
        });
        self.version = self.version.saturating_add(1);
        self.status = PlanStatus::Draft;
        Ok(())
    }

    pub fn render(&self) -> String {
        let mut out = format!(
            "Plan {} v{} [{}]\nObjective: {}\n",
            short_id(&self.id),
            self.version,
            match self.status {
                PlanStatus::Draft => "draft",
                PlanStatus::Approved => "approved",
            },
            self.objective
        );
        if !self.constraints.is_empty() {
            out.push_str("Constraints:\n");
            for c in &self.constraints {
                out.push_str(&format!("  - {}\n", c.text));
            }
        }
        for step in &self.steps {
            let mark = if step.done { 'x' } else { ' ' };
            out.push_str(&format!("[{mark}] {}\n", step.text));
        }
        let done = self.steps.iter().filter(|s| s.done).count();
        out.push_str(&format!("Progress: {done}/{}", self.steps.len()));
        out
    }
}

fn short_id(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

fn parse_objective_and_steps(
    objective: &str,
    steps_text: &str,
) -> Result<(String, Vec<PlanStep>), String> {
    let objective = objective.trim();
    if objective.is_empty() {
        return Err("update_plan objective must not be empty".to_string());
    }

    let mut steps = Vec::new();
    for line in steps_text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let (done, rest) = parse_step_line(line);
        let text = rest.trim().trim_start_matches(['-', '*']).trim();
        if text.is_empty() {
            continue;
        }
        steps.push(PlanStep {
            text: text.to_string(),
            done,
        });
        if steps.len() > AgentPlan::MAX_STEPS {
            return Err(format!(
                "update_plan supports at most {} steps",
                AgentPlan::MAX_STEPS
            ));
        }
    }

    if steps.is_empty() {
        return Err("update_plan steps must list at least one step".to_string());
    }

    Ok((objective.to_string(), steps))
}

fn parse_step_line(line: &str) -> (bool, &str) {
    let trimmed = line.trim_start();
    let lower = trimmed.to_ascii_lowercase();

    let prefixes: &[(&str, bool)] = &[
        ("- [x]", true),
        ("- [ ]", false),
        ("* [x]", true),
        ("* [ ]", false),
        ("[x]", true),
        ("[ ]", false),
        ("[]", false),
    ];

    for (prefix, done) in prefixes {
        let plen = prefix.len();
        if lower.len() >= plen && lower.as_bytes()[..plen] == prefix.as_bytes()[..plen] {
            return (*done, &trimmed[plen..]);
        }
    }

    if lower.starts_with("[x]") {
        return (true, &trimmed[3..]);
    }

    (false, trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_checklist_and_renders() {
        let plan = AgentPlan::from_update(
            "Fix auth redirect",
            "[x] Locate frontend auth\n[ ] Inspect backend\nFix token validation\n",
        )
        .unwrap();
        assert_eq!(plan.objective, "Fix auth redirect");
        assert_eq!(plan.version, 1);
        assert_eq!(plan.status, PlanStatus::Draft);
        assert_eq!(plan.steps.len(), 3);
        assert!(plan.steps[0].done);
        assert!(!plan.steps[1].done);
        assert!(!plan.steps[2].done);
        assert_eq!(plan.steps[2].text, "Fix token validation");

        let rendered = plan.render();
        assert!(rendered.contains("Objective: Fix auth redirect"));
        assert!(rendered.contains("[x] Locate frontend auth"));
        assert!(rendered.contains("[ ] Inspect backend"));
        assert!(rendered.contains("Progress: 1/3"));
        assert!(rendered.contains("[draft]"));
    }

    #[test]
    fn rejects_empty() {
        assert!(AgentPlan::from_update("", "a").is_err());
        assert!(AgentPlan::from_update("obj", "  \n  ").is_err());
    }

    #[test]
    fn amend_bumps_version_and_resets_to_draft() {
        let mut plan = AgentPlan::from_update("obj", "[ ] a\n[ ] b").unwrap();
        let id = plan.id.clone();
        plan.approve();
        plan.add_constraint("frontend only").unwrap();
        assert_eq!(plan.version, 2);
        assert_eq!(plan.status, PlanStatus::Draft);

        plan.amend_from_update("obj2", "[x] a\n[ ] b\n[ ] c").unwrap();
        assert_eq!(plan.id, id);
        assert_eq!(plan.version, 3);
        assert_eq!(plan.status, PlanStatus::Draft);
        assert_eq!(plan.objective, "obj2");
        assert_eq!(plan.steps.len(), 3);
        assert_eq!(plan.constraints.len(), 1);
        assert_eq!(plan.constraints[0].text, "frontend only");
    }

    #[test]
    fn approve_sets_status() {
        let mut plan = AgentPlan::from_update("obj", "[ ] a").unwrap();
        plan.approve();
        assert_eq!(plan.status, PlanStatus::Approved);
        assert!(plan.render().contains("[approved]"));
    }
}
