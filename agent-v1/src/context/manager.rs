use std::path::Path;

use serde::Serialize;

use crate::context::{ContextBuilder, ContextStats};
use crate::session::{CompactionState, Message, Session};
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
    keep_recent_session_messages: usize,
}

impl ContextManager {
    pub fn new(
        max_context_tokens: usize,
        reserve_tokens: usize,
        keep_recent_session_messages: usize,
    ) -> Self {
        Self {
            max_context_tokens,
            reserve_tokens,
            keep_recent_session_messages,
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
        session: Option<&mut Session>,
        session_messages: &[Message],
        step_history: &[Step],
    ) -> (
        crate::context::LlmRequest,
        ContextStats,
        ContextStats,
        ContextReduction,
    ) {
        let budget = self.budget_tokens();

        // Use persisted compaction view (if present) as the baseline view.
        let (summary_text, summarized_until) = session
            .as_ref()
            .and_then(|s| s.compaction.as_ref())
            .map(|c| (Some(c.summary.as_str()), c.summarized_until))
            .unwrap_or((None, 0usize));

        let summarized_until = summarized_until.min(session_messages.len());
        let base_messages = &session_messages[summarized_until..];

        let mut full_view: Vec<Message> = Vec::new();
        if let Some(text) = summary_text {
            if !text.trim().is_empty() {
                full_view.push(Message::summary(text));
            }
        }
        full_view.extend_from_slice(base_messages);

        let (full_request, full_stats) =
            builder.build_with_stats(workspace_root, goal, &full_view, step_history);

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

        // Trim: prefer dropping old messages, keep recent messages.
        let len = base_messages.len();
        let mut start = len.saturating_sub(self.keep_recent_session_messages);
        let mut best = (full_request.clone(), full_stats.clone(), 0usize);

        while start <= len {
            let dropped_chunk = &base_messages[..start];
            let kept_slice = &base_messages[start..];

            let chunk_summary = summarize_messages_chunk(dropped_chunk);
            let merged_summary = merge_summary(summary_text, chunk_summary.as_deref());

            let mut view: Vec<Message> = Vec::new();
            if !merged_summary.trim().is_empty() {
                view.push(Message::summary(&merged_summary));
            }
            view.extend_from_slice(kept_slice);

            let (req, stats) =
                builder.build_with_stats(workspace_root, goal, &view, step_history);

            let dropped = start;
            best = (req, stats.clone(), dropped);

            if stats.estimated_tokens <= budget {
                break;
            }

            if start == len {
                break;
            }
            start += 1;
        }

        let (request, stats, dropped) = best;

        // Persist compaction state (summary + cutoff) if we trimmed and can mutate session.
        if dropped > 0 {
            if let Some(session) = session {
                let new_summarized_until = (summarized_until + dropped).min(session_messages.len());
                let chunk = &session_messages[summarized_until..new_summarized_until];
                let chunk_summary = summarize_messages_chunk(chunk);
                let merged_summary = merge_summary(
                    session.compaction.as_ref().map(|c| c.summary.as_str()),
                    chunk_summary.as_deref(),
                );

                session.compaction = Some(CompactionState {
                    summarized_until: new_summarized_until,
                    summary: merged_summary,
                });
            }
        }

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

fn summarize_messages_chunk(chunk: &[Message]) -> Option<String> {
    // Deterministic, simple summary for now (no extra LLM call).
    // Keep it short and bounded.
    const MAX_CHARS: usize = 1200;
    const MAX_LINES: usize = 30;

    if chunk.is_empty() {
        return None;
    }

    let mut lines: Vec<String> = Vec::new();
    for msg in chunk {
        match msg {
            Message::User { content } => lines.push(format!("User: {}", content.trim())),
            Message::Assistant { content } => {
                lines.push(format!("Assistant: {}", content.trim()))
            }
            Message::Tool { name, result } => lines.push(format!(
                "Tool({}): {}",
                name,
                result.lines().next().unwrap_or("").trim()
            )),
            Message::Summary { content } => lines.push(format!("Summary: {}", content.trim())),
        }
        if lines.len() >= MAX_LINES {
            break;
        }
    }

    let mut out = lines.join("\n");
    if out.len() > MAX_CHARS {
        out.truncate(MAX_CHARS);
        out.push_str("…");
    }
    Some(out)
}

fn merge_summary(existing: Option<&str>, new_chunk: Option<&str>) -> String {
    let existing = existing.unwrap_or("").trim();
    let new_chunk = new_chunk.unwrap_or("").trim();

    if existing.is_empty() {
        return new_chunk.to_string();
    }
    if new_chunk.is_empty() {
        return existing.to_string();
    }

    let mut merged = String::new();
    merged.push_str(existing);
    merged.push_str("\n---\n");
    merged.push_str(new_chunk);

    const MAX: usize = 2000;
    if merged.len() > MAX {
        merged.truncate(MAX);
        merged.push_str("…");
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{Message, Session, SessionSettings};

    #[test]
    fn trims_oldest_session_messages_when_over_budget() {
        let dir = tempfile::tempdir().unwrap();
        let builder = ContextBuilder::new();
        let mgr = ContextManager::new(100, 0, 20); // tiny budget

        let mut msgs = Vec::new();
        for i in 0..50 {
            msgs.push(Message::user(format!("msg {i} - {}", "x".repeat(50))));
        }

        let (_req, full, bounded, reduction) =
            mgr.build_bounded_request(&builder, dir.path(), "goal", None, &msgs, &[]);

        assert!(full.estimated_tokens > mgr.budget_tokens());
        assert!(bounded.estimated_tokens <= full.estimated_tokens);
        assert!(reduction.trimmed);
        assert!(reduction.dropped_session_messages > 0);
    }

    #[test]
    fn injects_summary_when_trimming() {
        let dir = tempfile::tempdir().unwrap();
        let builder = ContextBuilder::new();
        let mgr = ContextManager::new(200, 0, 20); // tiny budget

        let mut msgs = Vec::new();
        for i in 0..50 {
            msgs.push(Message::user(format!("msg {i} - {}", "x".repeat(50))));
        }

        let (req, _full, _bounded, reduction) =
            mgr.build_bounded_request(&builder, dir.path(), "goal", None, &msgs, &[]);
        assert!(reduction.trimmed);

        let arr = req.messages.as_array().unwrap();
        let has_summary = arr.iter().any(|m| {
            m.get("role").and_then(|r| r.as_str()) == Some("assistant")
                && m
                    .get("content")
                    .and_then(|c| c.as_str())
                    .unwrap_or("")
                    .contains("Session summary:")
        });
        assert!(has_summary);
    }

    #[test]
    fn prefers_keeping_recent_messages() {
        let dir = tempfile::tempdir().unwrap();
        let builder = ContextBuilder::new();
        let mgr = ContextManager::new(4000, 0, 5); // keep last 5 if possible

        let mut msgs = Vec::new();
        for i in 0..30 {
            msgs.push(Message::user(format!("old {i} {}", "x".repeat(50))));
        }
        // make last messages distinct
        msgs.push(Message::user("RECENT A"));
        msgs.push(Message::user("RECENT B"));
        msgs.push(Message::user("RECENT C"));
        msgs.push(Message::user("RECENT D"));
        msgs.push(Message::user("RECENT E"));

        let (req, _full, _bounded, _reduction) =
            mgr.build_bounded_request(&builder, dir.path(), "goal", None, &msgs, &[]);
        // If trimming happens, the policy should prefer keeping the most recent messages.

        let arr = req.messages.as_array().unwrap();
        let joined = arr
            .iter()
            .filter_map(|m| m.get("content").and_then(|c| c.as_str()))
            .collect::<Vec<_>>()
            .join("\n");

        // Expect recent messages to survive into context view.
        assert!(joined.contains("RECENT A"));
        assert!(joined.contains("RECENT E"));
    }

    #[test]
    fn persists_compaction_state_when_session_is_mutable() {
        let dir = tempfile::tempdir().unwrap();
        let builder = ContextBuilder::new();
        let mgr = ContextManager::new(200, 0, 5);

        let mut session = Session::new(dir.path(), SessionSettings::default());
        for i in 0..50 {
            session.add_user_message(format!("msg {i} - {}", "x".repeat(50)));
        }

        let messages_snapshot = session.messages.clone();
        let (_req, _full, _bounded, reduction) = mgr.build_bounded_request(
            &builder,
            dir.path(),
            "goal",
            Some(&mut session),
            &messages_snapshot,
            &[],
        );

        assert!(reduction.trimmed);
        let c = session.compaction.as_ref().unwrap();
        assert!(c.summarized_until > 0);
        assert!(!c.summary.trim().is_empty());
    }
}

