//! Turn intent — declarative metadata only (Phase 12).
//!
//! Describes what the agent is currently doing for UI / observability / light
//! prompting. [`crate::authorization::EditPolicy`] remains the safety authority;
//! intent must never disable tools by itself.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TurnIntent {
    #[default]
    Explore,
    Plan,
    Execute,
    Review,
}

impl TurnIntent {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Explore => "explore",
            Self::Plan => "plan",
            Self::Execute => "execute",
            Self::Review => "review",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "explore" | "exploring" | "investigate" => Some(Self::Explore),
            "plan" | "planning" => Some(Self::Plan),
            "execute" | "exec" | "implement" | "fix" | "edit" => Some(Self::Execute),
            "review" | "verify" | "check" => Some(Self::Review),
            _ => None,
        }
    }

    /// Best-effort inference from a user message. Conservative defaults to Explore.
    ///
    /// This is metadata only — never used to authorize or deny tools.
    pub fn infer_from_message(message: &str) -> Self {
        let lower = message.to_ascii_lowercase();
        let lower = lower.trim();

        // Explicit prefixes take priority if present in prose.
        if lower.contains("review") || lower.contains("verify") || lower.contains("check the diff")
        {
            return Self::Review;
        }
        if lower.contains("propose a plan")
            || lower.contains("make a plan")
            || lower.starts_with("plan ")
            || lower == "plan"
            || lower.contains("what's the plan")
        {
            return Self::Plan;
        }
        if lower.contains("fix ")
            || lower.contains("implement")
            || lower.contains("apply the")
            || lower.contains("proceed")
            || lower.contains("make the change")
            || lower.contains("edit ")
        {
            return Self::Execute;
        }
        if lower.contains("explain")
            || lower.contains("what is this")
            || lower.contains("overview")
            || lower.contains("investigate")
            || lower.contains("find ")
            || lower.contains("where's")
            || lower.contains("where is")
        {
            return Self::Explore;
        }

        Self::Explore
    }

    pub fn prompt_hint(self) -> &'static str {
        match self {
            Self::Explore => {
                "Declared intent: explore (metadata). Prefer locate/summarize_repo/read; \
                 EditPolicy still governs any mutations."
            }
            Self::Plan => {
                "Declared intent: plan (metadata). Prefer update_plan and investigation; \
                 EditPolicy still governs any mutations — tools are not disabled."
            }
            Self::Execute => {
                "Declared intent: execute (metadata). Implementation may proceed subject to EditPolicy."
            }
            Self::Review => {
                "Declared intent: review (metadata). Prefer git_diff/git_status/verification; \
                 EditPolicy still governs any mutations."
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_infers() {
        assert_eq!(TurnIntent::parse("plan"), Some(TurnIntent::Plan));
        assert_eq!(TurnIntent::parse("nope"), None);
        assert_eq!(
            TurnIntent::infer_from_message("Explain this repo briefly"),
            TurnIntent::Explore
        );
        assert_eq!(
            TurnIntent::infer_from_message("Propose a plan for the date bug"),
            TurnIntent::Plan
        );
        assert_eq!(
            TurnIntent::infer_from_message("Proceed and fix the frontend"),
            TurnIntent::Execute
        );
        assert_eq!(
            TurnIntent::infer_from_message("Review the diff and verify"),
            TurnIntent::Review
        );
    }
}
