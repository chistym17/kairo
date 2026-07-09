use std::path::Path;

use serde::Serialize;

use crate::context::{ContextBuilder, ContextStats};
use crate::session::Message;
use crate::state::step::Step;

/// What was reduced to fit the budget (Phase 6.2).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ContextReduction {
    pub trimmed: bool,
    pub dropped_session_messages: usize,
    pub budget_tokens: usize,
}

/// Produces a bounded "view" of a session for the model.
///
/// Phase 6.2 keeps this intentionally simple:
/// - If over budget, drop oldest session messages until it fits.
/// - Never mutates the stored session; only affects what is sent to the LLM.
#[derive(Debug, Clone)]
pub struct ContextManager {
    max_context_tokens: usize,
    reserve_tokens: usize,
}

impl ContextManager {
    pub fn new(max_context_tokens: usize, reserve_tokens: usize) -> Self {
        Self {
            max_context_tokens,
            reserve_tokens,
        }
    }

    pub fn budget_tokens(&self) -> usize {
        self.max_context_tokens.saturating_sub(self.reserve_tokens)
    }

    /// Build a request and (if needed) trim session history to fit budget.
    pub fn build_bounded_request(
        &self,
        builder: &ContextBuilder,
        workspace_root: &Path,
        goal: &str,
        session_messages: &[Message],
        step_history: &[Step],
    ) -> (
        crate::context::LlmRequest,
        ContextStats,
        ContextStats,
        ContextReduction,
    ) {
        let budget = self.budget_tokens();

        // Baseline stats with full session view.
        let (full_request, full_stats) =
            builder.build_with_stats(workspace_root, goal, session_messages, step_history);

        if full_stats.estimated_tokens <= budget {
            return (
                full_request,
                full_stats.clone(),
                full_stats,
                ContextReduction {
                    trimmed: false,
                    dropped_session_messages: 0,
                    budget_tokens: budget,
                },
            );
        }

        // Trim: drop oldest session messages first.
        let mut start = 0usize;
        let mut best = (full_request.clone(), full_stats.clone(), 0usize);

        while start < session_messages.len() {
            start += 1;
            let view = &session_messages[start..];
            let (req, stats) = builder.build_with_stats(workspace_root, goal, view, step_history);

            let dropped = start;
            best = (req, stats.clone(), dropped);

            if stats.estimated_tokens <= budget {
                break;
            }
        }

        let (request, stats, dropped) = best;

        (
            request,
            full_stats,
            stats,
            ContextReduction {
                trimmed: true,
                dropped_session_messages: dropped,
                budget_tokens: budget,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::Message;

    #[test]
    fn trims_oldest_session_messages_when_over_budget() {
        let dir = tempfile::tempdir().unwrap();
        let builder = ContextBuilder::new();
        let mgr = ContextManager::new(100, 0); // tiny budget

        let mut msgs = Vec::new();
        for i in 0..50 {
            msgs.push(Message::user(format!("msg {i} - {}", "x".repeat(50))));
        }

        let (_req, full, bounded, reduction) =
            mgr.build_bounded_request(&builder, dir.path(), "goal", &msgs, &[]);

        assert!(full.estimated_tokens > mgr.budget_tokens());
        assert!(bounded.estimated_tokens <= full.estimated_tokens);
        assert!(reduction.trimmed);
        assert!(reduction.dropped_session_messages > 0);
    }
}

