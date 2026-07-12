use serde::{Deserialize, Serialize};

/// Lightweight per-run plan kept in agent state and injected into context.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentPlan {
    pub objective: String,
    pub steps: Vec<PlanStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanStep {
    pub text: String,
    pub done: bool,
}

impl AgentPlan {
    pub const MAX_STEPS: usize = 20;

    /// Build a plan from an objective + newline-separated checklist.
    /// Lines may use `[x]` / `[ ]` prefixes; bare lines are incomplete.
    pub fn from_update(objective: &str, steps_text: &str) -> Result<Self, String> {
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
            if steps.len() > Self::MAX_STEPS {
                return Err(format!(
                    "update_plan supports at most {} steps",
                    Self::MAX_STEPS
                ));
            }
        }

        if steps.is_empty() {
            return Err("update_plan steps must list at least one step".to_string());
        }

        Ok(Self {
            objective: objective.to_string(),
            steps,
        })
    }

    pub fn render(&self) -> String {
        let mut out = format!("Objective: {}\n", self.objective);
        for step in &self.steps {
            let mark = if step.done { 'x' } else { ' ' };
            out.push_str(&format!("[{mark}] {}\n", step.text));
        }
        let done = self.steps.iter().filter(|s| s.done).count();
        out.push_str(&format!("Progress: {done}/{}", self.steps.len()));
        out
    }
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
            // Match against original length using the lowercase prefix length
            return (*done, &trimmed[plen..]);
        }
    }

    // Case-insensitive [x] when written as [X]
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
    }

    #[test]
    fn rejects_empty() {
        assert!(AgentPlan::from_update("", "a").is_err());
        assert!(AgentPlan::from_update("obj", "  \n  ").is_err());
    }
}
