//! Evidence progress + decision checkpoint (Phase 12 hardening).
//!
//! Distinguishes *tool success* from *new evidence*. Repeated searches over
//! already-seen matches do not reset stagnation. After a no-decision budget is
//! exhausted, exploration is restricted until the model plans, edits, or finishes.

use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use crate::state::step::Action;
use crate::tools::ToolResult;

/// Soft warning after this many consecutive no-progress actions.
pub const NO_PROGRESS_WARN_AFTER: u32 = 3;
/// Stronger soft guidance.
pub const NO_PROGRESS_DENY_DUP_AFTER: u32 = 6;
/// Soft “must checkpoint” guidance (still advisory).
pub const NO_PROGRESS_CHECKPOINT_AFTER: u32 = 8;
/// Hard gate: consecutive no-progress actions activate the checkpoint.
pub const HARD_CHECKPOINT_NO_PROGRESS: u32 = 8;
/// Hard gate: exploration tools without a decision (plan/edit/finish).
pub const EXPLORATION_CEILING: u32 = 30;
/// After checkpoint, one further exploration action is allowed (then deny).
pub const JUSTIFIED_EXPLORATION_SLOTS: u32 = 1;

/// What an observed tool contributed (reasoning progress, not HTTP success).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressKind {
    NewEvidence,
    PlanUpdated,
    Mutation,
    VerificationResult,
    Decision,
    NoProgress,
}

/// Stable fingerprint of a retrieved evidence atom.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EvidenceKey {
    pub path: String,
    pub line_start: u32,
    pub line_end: u32,
    pub content_hash: u64,
}

#[derive(Debug, Default, Clone)]
pub struct ProgressState {
    pub consecutive_no_progress: u32,
    pub duplicate_reads: u32,
    pub unique_files_read: HashSet<PathBuf>,
    pub evidence_keys: HashSet<EvidenceKey>,
    pub new_evidence_count: u32,
    pub mutations: u32,
    pub plan_updates: u32,
    /// Read/search/locate/find_files since last decision (plan/edit/finish).
    pub exploration_without_decision: u32,
    /// When true, generic exploration is restricted.
    pub checkpoint_active: bool,
    /// Remaining “one more look” slots while checkpoint is active.
    pub justified_exploration_slots: u32,
    pub checkpoint_activations: u32,
    pub checkpoint_denials: u32,
    pub stagnation_nudges: u32,
    last_nudge_tier: Option<u32>,
    /// Queries that already returned empty (query + scope).
    empty_retrievals: HashSet<String>,
}

impl ProgressState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn note_edit(&mut self) {
        self.mutations = self.mutations.saturating_add(1);
        self.on_decision(ProgressKind::Mutation);
    }

    pub fn note_plan_update(&mut self) {
        self.plan_updates = self.plan_updates.saturating_add(1);
        self.on_decision(ProgressKind::PlanUpdated);
    }

    fn on_decision(&mut self, _kind: ProgressKind) {
        self.consecutive_no_progress = 0;
        self.exploration_without_decision = 0;
        self.checkpoint_active = false;
        self.justified_exploration_slots = JUSTIFIED_EXPLORATION_SLOTS;
        self.last_nudge_tier = None;
        self.new_evidence_count = self.new_evidence_count.saturating_add(1);
    }

    fn mark_new_evidence(&mut self) {
        self.consecutive_no_progress = 0;
        self.new_evidence_count = self.new_evidence_count.saturating_add(1);
        self.last_nudge_tier = None;
        // New evidence does **not** clear exploration_without_decision or checkpoint.
        self.maybe_activate_checkpoint();
    }

    fn mark_no_progress(&mut self) {
        self.consecutive_no_progress = self.consecutive_no_progress.saturating_add(1);
        self.maybe_activate_checkpoint();
    }

    fn maybe_activate_checkpoint(&mut self) {
        if self.checkpoint_active {
            return;
        }
        if self.consecutive_no_progress >= HARD_CHECKPOINT_NO_PROGRESS
            || self.exploration_without_decision >= EXPLORATION_CEILING
        {
            self.checkpoint_active = true;
            self.justified_exploration_slots = JUSTIFIED_EXPLORATION_SLOTS;
            self.checkpoint_activations = self.checkpoint_activations.saturating_add(1);
        }
    }

    fn note_exploration_action(&mut self) {
        self.exploration_without_decision =
            self.exploration_without_decision.saturating_add(1);
        self.maybe_activate_checkpoint();
    }

    /// Gate before executing exploration tools. `Ok(())` = allow.
    pub fn check_exploration_allowed(&mut self, action: &Action) -> Result<(), String> {
        if !self.checkpoint_active {
            return Ok(());
        }
        if is_decision_action(action) || is_review_or_verify_action(action) {
            return Ok(());
        }
        if !is_exploration_action(action) {
            return Ok(());
        }

        if self.justified_exploration_slots > 0 {
            self.justified_exploration_slots -= 1;
            return Ok(());
        }

        self.checkpoint_denials = self.checkpoint_denials.saturating_add(1);
        Err(self.checkpoint_denial_message())
    }

    pub fn checkpoint_denial_message(&self) -> String {
        format!(
            "Exploration checkpoint active (no-progress streak={}, exploration_without_decision={}/{}).\n\
             Further generic search/read is blocked until you make a decision.\n\
             Choose one:\n\
             - update_plan with a concrete checklist;\n\
             - propose or apply the fix (apply_patch / write_file);\n\
             - finish with current findings;\n\
             - (if you already used your one justified look) state the single missing fact in the plan, then inspect only that.\n\
             Do not repeat searches for evidence already in context.",
            self.consecutive_no_progress, self.exploration_without_decision, EXPLORATION_CEILING
        )
    }

    pub fn checkpoint_guidance_for_injection(&self) -> Option<String> {
        if !self.checkpoint_active {
            return None;
        }
        Some(format!(
            "Evidence checkpoint: exploration is restricted. \
             no-progress streak={}, exploration_without_decision={}/{}, justified_looks_left={}.\n\
             You must decide: update_plan, apply_patch/write_file, or finish with findings. \
             Only one specifically justified new exploration action is allowed while this is active.",
            self.consecutive_no_progress,
            self.exploration_without_decision,
            EXPLORATION_CEILING,
            self.justified_exploration_slots
        ))
    }

    /// Observe one completed tool. Returns the progress kind recorded.
    pub fn observe_tool(
        &mut self,
        action: &Action,
        result: &ToolResult,
        duplicate_read: bool,
    ) -> ProgressKind {
        let kind = self.classify_and_ingest(action, result, duplicate_read);
        match kind {
            ProgressKind::Mutation => self.note_edit(),
            ProgressKind::PlanUpdated => self.note_plan_update(),
            ProgressKind::Decision => self.on_decision(ProgressKind::Decision),
            ProgressKind::NewEvidence | ProgressKind::VerificationResult => {
                if is_exploration_action(action) {
                    self.note_exploration_action();
                }
                self.mark_new_evidence();
            }
            ProgressKind::NoProgress => {
                if is_exploration_action(action) {
                    self.note_exploration_action();
                }
                self.mark_no_progress();
            }
        }
        // note_edit/note_plan_update already called on_decision; avoid double for Mutation/Plan
        // — classify returns those and we call note_* which calls on_decision. Good.
        // But wait: for Mutation we call note_edit which increments mutations again if observe
        // is only path... note_edit increments mutations. classify returns Mutation, then
        // note_edit() — once. Good.
        kind
    }

    fn classify_and_ingest(
        &mut self,
        action: &Action,
        result: &ToolResult,
        duplicate_read: bool,
    ) -> ProgressKind {
        match action {
            Action::ApplyPatch { .. } | Action::WriteFile { .. } if result.success => {
                ProgressKind::Mutation
            }
            Action::UpdatePlan { .. } if result.success => ProgressKind::PlanUpdated,
            Action::Finish { .. } => ProgressKind::Decision,
            Action::ReadFile { path, offset, limit } => {
                if duplicate_read || is_duplicate_read_result(result) {
                    self.duplicate_reads = self.duplicate_reads.saturating_add(1);
                    return ProgressKind::NoProgress;
                }
                if !result.success {
                    return ProgressKind::NoProgress;
                }
                let _ = self.unique_files_read.insert(normalize_path(path));
                let keys = evidence_from_read(path, *offset, *limit, &result.output);
                if self.ingest_keys(keys) {
                    ProgressKind::NewEvidence
                } else {
                    ProgressKind::NoProgress
                }
            }
            Action::SearchCode { query, path } => {
                if !result.success {
                    return ProgressKind::NoProgress;
                }
                if result.output.contains("(no matches)") {
                    let empty_key = format!("search\0{}\0{}", query, path.display());
                    if self.empty_retrievals.insert(empty_key) {
                        // First empty for this query/scope: weak signal, still no positive evidence.
                        // Count as NoProgress so search thrash cannot reset the streak.
                        return ProgressKind::NoProgress;
                    }
                    return ProgressKind::NoProgress;
                }
                let keys = evidence_from_search(&result.output);
                if self.ingest_keys(keys) {
                    ProgressKind::NewEvidence
                } else {
                    ProgressKind::NoProgress
                }
            }
            Action::Locate { query, path } => {
                if !result.success {
                    return ProgressKind::NoProgress;
                }
                if result.output.contains("(no candidates)") {
                    let empty_key = format!("locate\0{}\0{}", query, path.display());
                    let _ = self.empty_retrievals.insert(empty_key);
                    return ProgressKind::NoProgress;
                }
                let keys = evidence_from_locate(&result.output);
                if self.ingest_keys(keys) {
                    ProgressKind::NewEvidence
                } else {
                    ProgressKind::NoProgress
                }
            }
            Action::FindFiles { pattern, path } => {
                if !result.success {
                    return ProgressKind::NoProgress;
                }
                if result.output.contains("(no matches)") {
                    let empty_key = format!("find\0{}\0{}", pattern, path.display());
                    let _ = self.empty_retrievals.insert(empty_key);
                    return ProgressKind::NoProgress;
                }
                let keys = evidence_from_find_files(&result.output);
                if self.ingest_keys(keys) {
                    ProgressKind::NewEvidence
                } else {
                    ProgressKind::NoProgress
                }
            }
            Action::ListFiles { path } => {
                if !result.success {
                    return ProgressKind::NoProgress;
                }
                let key = EvidenceKey {
                    path: format!("list:{}", path.display()),
                    line_start: 0,
                    line_end: 0,
                    content_hash: hash_str(&result.output),
                };
                if self.ingest_keys(vec![key]) {
                    ProgressKind::NewEvidence
                } else {
                    ProgressKind::NoProgress
                }
            }
            Action::SummarizeRepo { .. } | Action::DetectProjectSnapshot { .. }
                if result.success =>
            {
                let key = EvidenceKey {
                    path: format!("meta:{}", tool_label(action)),
                    line_start: 0,
                    line_end: 0,
                    content_hash: hash_str(&result.output),
                };
                if self.ingest_keys(vec![key]) {
                    ProgressKind::NewEvidence
                } else {
                    ProgressKind::NoProgress
                }
            }
            Action::RunCommand { .. } | Action::RunShell { .. } if result.success => {
                if result
                    .error
                    .as_deref()
                    .map(|e| e.to_ascii_lowercase().contains("blocked"))
                    .unwrap_or(false)
                {
                    ProgressKind::NoProgress
                } else {
                    ProgressKind::VerificationResult
                }
            }
            Action::GitDiff { .. } | Action::GitStatus if result.success => {
                // First review in a streak can be evidence; repeating while stagnating is not.
                if self.consecutive_no_progress >= NO_PROGRESS_WARN_AFTER {
                    ProgressKind::NoProgress
                } else {
                    let key = EvidenceKey {
                        path: format!("git:{}", tool_label(action)),
                        line_start: 0,
                        line_end: 0,
                        content_hash: hash_str(&result.output),
                    };
                    if self.ingest_keys(vec![key]) {
                        ProgressKind::NewEvidence
                    } else {
                        ProgressKind::NoProgress
                    }
                }
            }
            _ => {
                if result.success {
                    ProgressKind::NewEvidence
                } else {
                    ProgressKind::NoProgress
                }
            }
        }
    }

    fn ingest_keys(&mut self, keys: Vec<EvidenceKey>) -> bool {
        let mut any_new = false;
        for key in keys {
            if self.evidence_keys.insert(key) {
                any_new = true;
            }
        }
        any_new
    }

    /// Soft stagnation guidance when checkpoint is not already messaging harder.
    pub fn nudge_for_injection(&mut self) -> Option<String> {
        if let Some(msg) = self.checkpoint_guidance_for_injection() {
            self.stagnation_nudges = self.stagnation_nudges.saturating_add(1);
            return Some(msg);
        }

        let n = self.consecutive_no_progress;
        let (tier, text) = if n >= NO_PROGRESS_CHECKPOINT_AFTER {
            (
                3u32,
                format!(
                    "Exploration has produced no new evidence for {n} actions. \
                     Make a decision: update_plan, apply_patch, ask the user, or finish with findings. \
                     Repeated searches of already-seen matches do not count as progress."
                ),
            )
        } else if n >= NO_PROGRESS_DENY_DUP_AFTER {
            (
                2u32,
                format!(
                    "No new evidence for {n} actions. Do not reread or re-search the same hits. \
                     Update the plan, edit, or finish with findings."
                ),
            )
        } else if n >= NO_PROGRESS_WARN_AFTER {
            (
                1u32,
                format!(
                    "You are repeating previously inspected evidence ({n} no-progress actions). \
                     Prefer a decision (plan / patch / finish) over another similar search."
                ),
            )
        } else {
            return None;
        };

        if self.last_nudge_tier == Some(tier) && tier < 3 {
            return Some(text);
        }
        self.last_nudge_tier = Some(tier);
        self.stagnation_nudges = self.stagnation_nudges.saturating_add(1);
        Some(text)
    }

    /// Soft guidance once a plan exists and no edit has landed yet.
    pub fn after_decision_guidance(&self) -> Option<String> {
        if self.mutations > 0 || self.plan_updates == 0 || self.checkpoint_active {
            return None;
        }
        Some(
            "A concrete change has been identified.\n\
             Prefer one coherent work unit:\n\
             1. reread only the exact edit range if needed;\n\
             2. apply_patch for that group only;\n\
             3. inspect git_diff;\n\
             4. run one verification;\n\
             5. if still red, pick the next coherent group — do not re-explore broadly.\n\
             Do not invent patch context. Do not edit files via shell (sed/redirects)."
                .to_string(),
        )
    }
}

fn is_exploration_action(action: &Action) -> bool {
    matches!(
        action,
        Action::ReadFile { .. }
            | Action::SearchCode { .. }
            | Action::Locate { .. }
            | Action::FindFiles { .. }
            | Action::ListFiles { .. }
            | Action::SummarizeRepo { .. }
            | Action::DetectProjectSnapshot { .. }
    )
}

fn is_decision_action(action: &Action) -> bool {
    matches!(
        action,
        Action::UpdatePlan { .. }
            | Action::ApplyPatch { .. }
            | Action::WriteFile { .. }
            | Action::Finish { .. }
    )
}

fn is_review_or_verify_action(action: &Action) -> bool {
    matches!(
        action,
        Action::GitStatus
            | Action::GitDiff { .. }
            | Action::RunCommand { .. }
            | Action::RunShell { .. }
    )
}

fn tool_label(action: &Action) -> &'static str {
    match action {
        Action::SummarizeRepo { .. } => "summarize_repo",
        Action::DetectProjectSnapshot { .. } => "detect_project_snapshot",
        Action::GitStatus => "git_status",
        Action::GitDiff { .. } => "git_diff",
        _ => "other",
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    PathBuf::from(path.to_string_lossy().replace('\\', "/"))
}

fn hash_str(s: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

fn is_duplicate_read_result(result: &ToolResult) -> bool {
    let blob = format!(
        "{}{}",
        result.output,
        result.error.as_deref().unwrap_or("")
    );
    let lower = blob.to_ascii_lowercase();
    lower.contains("[already read")
        || lower.contains("exact content already available")
        || lower.contains("duplicate read denied")
        || lower.contains("exploration checkpoint active")
}

fn evidence_from_read(path: &Path, offset: u32, limit: u32, output: &str) -> Vec<EvidenceKey> {
    let body = output
        .lines()
        .skip_while(|l| l.starts_with("lines ") || l.starts_with('['))
        .collect::<Vec<_>>()
        .join("\n");
    let end = offset.saturating_add(limit.saturating_sub(1));
    vec![EvidenceKey {
        path: normalize_path(path).to_string_lossy().into_owned(),
        line_start: offset,
        line_end: end,
        content_hash: hash_str(&body),
    }]
}

/// Parse `path:line: snippet` lines from search_code output.
fn evidence_from_search(output: &str) -> Vec<EvidenceKey> {
    let mut keys = Vec::new();
    for line in output.lines() {
        if line.starts_with("search_code") || line.starts_with("(no matches)") {
            continue;
        }
        if let Some(key) = parse_path_line_snippet(line) {
            keys.push(key);
        }
    }
    keys
}

fn parse_path_line_snippet(line: &str) -> Option<EvidenceKey> {
    // Expect `rel/path.ext:LINE: snippet` — take the last `:digits:` before snippet.
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b':' {
            let rest = &line[i + 1..];
            if let Some((num, after)) = split_line_number(rest) {
                let path = line[..i].trim();
                if !path.is_empty() {
                    return Some(EvidenceKey {
                        path: path.replace('\\', "/"),
                        line_start: num,
                        line_end: num,
                        content_hash: hash_str(after.trim()),
                    });
                }
            }
        }
        i += 1;
    }
    None
}

fn split_line_number(s: &str) -> Option<(u32, &str)> {
    let (num_str, rest) = s.split_once(':')?;
    let num: u32 = num_str.parse().ok()?;
    Some((num, rest))
}

/// Parse locate candidate lines: `N. path:line  score=` or `N. path  score=`.
fn evidence_from_locate(output: &str) -> Vec<EvidenceKey> {
    let mut keys = Vec::new();
    for line in output.lines() {
        let trimmed = line.trim();
        if !trimmed
            .bytes()
            .next()
            .map(|b| b.is_ascii_digit())
            .unwrap_or(false)
        {
            continue;
        }
        // "1. backend/tasks/views.py:28  score=..."
        let Some(after_dot) = trimmed.split_once('.').map(|(_, r)| r.trim()) else {
            continue;
        };
        let loc = after_dot.split_whitespace().next().unwrap_or("");
        if loc.is_empty() {
            continue;
        }
        let (path, line_no) = if let Some((p, n)) = loc.rsplit_once(':') {
            if let Ok(num) = n.parse::<u32>() {
                (p, num)
            } else {
                (loc, 0)
            }
        } else {
            (loc, 0)
        };
        keys.push(EvidenceKey {
            path: path.replace('\\', "/"),
            line_start: line_no,
            line_end: line_no,
            content_hash: hash_str(loc),
        });
    }
    keys
}

fn evidence_from_find_files(output: &str) -> Vec<EvidenceKey> {
    let mut keys = Vec::new();
    for line in output.lines() {
        if line.starts_with("find_files") || line.starts_with("(no matches)") {
            continue;
        }
        let path = line.trim();
        if path.is_empty() {
            continue;
        }
        keys.push(EvidenceKey {
            path: path.replace('\\', "/"),
            line_start: 0,
            line_end: 0,
            content_hash: hash_str(path),
        });
    }
    keys
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(path: &str) -> Action {
        Action::ReadFile {
            path: PathBuf::from(path),
            offset: 1,
            limit: 50,
        }
    }

    fn search(query: &str) -> Action {
        Action::SearchCode {
            query: query.into(),
            path: PathBuf::from("."),
        }
    }

    fn ok_lines(body: &str) -> ToolResult {
        ToolResult {
            success: true,
            output: format!("lines 1-50 of 50 — a.rs\n{body}"),
            diff: None,
            error: None,
            stop_reason: None,
        }
    }

    fn search_hits(body: &str) -> ToolResult {
        ToolResult {
            success: true,
            output: format!("search_code query=\"x\" under '.' — 2 match(es)\n{body}"),
            diff: None,
            error: None,
            stop_reason: None,
        }
    }

    fn denied_dup() -> ToolResult {
        ToolResult {
            success: false,
            output: String::new(),
            diff: None,
            error: Some(
                "Duplicate read denied: Exact content already available in current context".into(),
            ),
            stop_reason: None,
        }
    }

    #[test]
    fn repeated_search_same_hits_is_no_progress() {
        let mut p = ProgressState::new();
        let hits = "frontend-v2/src/lib/api.ts:194: getTasks\nbackend/tasks/views.py:28: date_param";
        let kind1 = p.observe_tool(&search("date"), &search_hits(hits), false);
        assert_eq!(kind1, ProgressKind::NewEvidence);
        assert_eq!(p.consecutive_no_progress, 0);

        let kind2 = p.observe_tool(&search("due_date"), &search_hits(hits), false);
        assert_eq!(kind2, ProgressKind::NoProgress);
        assert_eq!(p.consecutive_no_progress, 1);
        assert!(!p.checkpoint_active);
    }

    #[test]
    fn duplicate_read_does_not_reset() {
        let mut p = ProgressState::new();
        p.observe_tool(&read("a.rs"), &ok_lines("hello"), false);
        assert_eq!(p.consecutive_no_progress, 0);
        p.observe_tool(&read("a.rs"), &denied_dup(), true);
        assert_eq!(p.consecutive_no_progress, 1);
        assert_eq!(p.duplicate_reads, 1);
    }

    #[test]
    fn hard_checkpoint_after_no_progress_budget() {
        let mut p = ProgressState::new();
        let hits = "a.rs:1: once";
        p.observe_tool(&search("a"), &search_hits(hits), false);
        for _ in 0..HARD_CHECKPOINT_NO_PROGRESS {
            p.observe_tool(&search("a"), &search_hits(hits), false);
        }
        assert!(p.checkpoint_active);
        assert!(p.check_exploration_allowed(&search("b")).is_ok()); // one justified slot
        assert!(p.check_exploration_allowed(&search("c")).is_err());
    }

    #[test]
    fn exploration_ceiling_activates_checkpoint() {
        let mut p = ProgressState::new();
        for i in 0..EXPLORATION_CEILING {
            let hits = format!("file{i}.rs:1: uniq");
            p.observe_tool(&search("q"), &search_hits(&hits), false);
        }
        assert!(p.checkpoint_active);
        assert!(p.exploration_without_decision >= EXPLORATION_CEILING);
    }

    #[test]
    fn plan_update_clears_checkpoint() {
        let mut p = ProgressState::new();
        p.checkpoint_active = true;
        p.justified_exploration_slots = 0;
        p.exploration_without_decision = 40;
        p.observe_tool(
            &Action::UpdatePlan {
                objective: "fix".into(),
                steps: "[ ] edit api".into(),
            },
            &ToolResult {
                success: true,
                output: "ok".into(),
                diff: None,
                error: None,
                stop_reason: None,
            },
            false,
        );
        assert!(!p.checkpoint_active);
        assert_eq!(p.exploration_without_decision, 0);
        assert!(p.check_exploration_allowed(&search("fresh")).is_ok());
    }

    #[test]
    fn after_decision_guidance_when_plan_and_no_edit() {
        let mut p = ProgressState::new();
        assert!(p.after_decision_guidance().is_none());
        p.observe_tool(
            &Action::UpdatePlan {
                objective: "fix".into(),
                steps: "[ ] edit".into(),
            },
            &ToolResult {
                success: true,
                output: "ok".into(),
                diff: None,
                error: None,
                stop_reason: None,
            },
            false,
        );
        let g = p.after_decision_guidance().unwrap();
        assert!(g.contains("apply_patch"));
        assert!(g.contains("do not re-explore broadly"));
    }
}
