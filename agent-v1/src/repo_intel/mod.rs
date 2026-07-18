//! Thin repository intelligence seam.
//!
//! Hybrid `locate` is the tool backend. [`SearchSnapshotIntel`] remains as a
//! legacy/test backend. Index is built once per run on [`AgentState::repo_intel`]
//! and updated incrementally after successful edits.

mod expand;
mod extractors;
mod hybrid;
mod indexer;
mod query;
mod search_snapshot;
mod snippets;
mod sources;
pub mod traits;
pub mod types;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub use expand::expand_imports;
pub use extractors::{
    CompositeSymbolExtractor, JsTsSymbolExtractor, PythonSymbolExtractor, RustSymbolExtractor,
};
pub use hybrid::{render_candidates, HybridIntel, LocateStats, HYBRID_TOP_K};
pub use indexer::{
    apply_changes, apply_changes_with, build_index, build_index_with, rebuild_lookups,
    DefaultFileIndexer, IndexBuildStats, IndexUpdateStats,
};
pub use query::{normalize_query, QueryTokens};
pub use search_snapshot::SearchSnapshotIntel;
pub use snippets::LineSnippetProvider;
pub use sources::{
    LexicalCandidateSource, PathCandidateSource, SymbolCandidateSource, TokenCandidateSource,
};
pub use traits::{
    CandidateMerger, CandidateRanker, CandidateSource, DefaultCandidateMerger, FileIndexer,
    HybridRepositoryIntelligence, IndexStorage, SignalSumRanker, SnippetProvider, SymbolExtractor,
};
pub use types::{
    CandidateSignal, CandidateSourceError, FileChange, FileChangeKind, FileHash, FileRecord,
    ImportReference, Language, LocateCandidate, LocateError, MergedCandidate, RawCandidate,
    RepositoryIndex, RepositoryIntelligenceError, RepositoryRevision, SignalKind, Snippet,
    SymbolKind, SymbolLocation, SymbolRecord,
};

use crate::tools::project_snapshot::ProjectSnapshot;
use crate::tools::ToolResult;

/// Soft caps so locate stays a compact observation, not a dump.
pub const MAX_CANDIDATES: usize = 12;
pub const MAX_SNIPPET_CHARS: usize = 160;
pub const MAX_RAW_HITS: usize = 80;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchReason {
    ExactMatch,
    DefinitionLike,
    PathMatch,
    EntryPoint,
    NearbyMatch,
}

impl MatchReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            MatchReason::ExactMatch => "exact_match",
            MatchReason::DefinitionLike => "definition_like",
            MatchReason::PathMatch => "path_match",
            MatchReason::EntryPoint => "entry_point",
            MatchReason::NearbyMatch => "nearby_match",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    pub path: PathBuf,
    /// 1-based line number (0 when the hit is path-only).
    pub line: usize,
    pub score: f32,
    pub reason: MatchReason,
    pub snippet: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LocateRequest {
    pub query: String,
    pub scope: PathBuf,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LocateResult {
    pub query: String,
    pub scope: PathBuf,
    pub candidates: Vec<Candidate>,
    pub truncated: bool,
}

impl LocateResult {
    pub fn render(&self) -> String {
        let mut out = format!(
            "locate query={:?} under '{}' — {} candidate(s){}\n",
            self.query,
            self.scope.display(),
            self.candidates.len(),
            if self.truncated {
                format!(" (capped at {MAX_CANDIDATES})")
            } else {
                String::new()
            }
        );

        if self.candidates.is_empty() {
            out.push_str("(no candidates)\n");
            return out;
        }

        for (i, c) in self.candidates.iter().enumerate() {
            let loc = if c.line > 0 {
                format!("{}:{}", c.path.display(), c.line)
            } else {
                c.path.display().to_string()
            };
            out.push_str(&format!(
                "\n{}. {}  score={:.1}  reason={}\n",
                i + 1,
                loc,
                c.score,
                c.reason.as_str()
            ));
            if let Some(snip) = &c.snippet {
                out.push_str("   ");
                out.push_str(snip);
                out.push('\n');
            }
        }
        out
    }
}

/// Legacy tool-facing locate (returns rendered [`LocateResult`]).
///
/// Hybrid ranked locate uses [`HybridRepositoryIntelligence`]. Additive
/// [`Self::update`] defaults to no-op so snapshot search stays valid.
pub trait RepositoryIntelligence {
    fn locate(&self, request: &LocateRequest) -> LocateResult;

    fn update(&mut self, _changes: &[FileChange]) -> Result<(), RepositoryIntelligenceError> {
        Ok(())
    }
}

/// Tool entry: ranked locate over workspace (optional path scopes search).
///
/// Builds a one-shot [`HybridIntel`] when callers have no run-scoped index
/// (tests / bare `tools::dispatch`). The agent loop prefers
/// [`execute_with_intel`] on the run's index.
pub fn execute(query: &str, path: &Path, workspace_root: &Path) -> ToolResult {
    if !workspace_root.exists() {
        return ToolResult::error(format!(
            "workspace_root does not exist: {}",
            workspace_root.display()
        ));
    }

    let q = query.trim();
    if q.is_empty() {
        return ToolResult::error("locate query must not be empty");
    }

    let scope = if path.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        path.to_path_buf()
    };

    let abs = workspace_root.join(&scope);
    if !abs.exists() {
        return ToolResult::error(format!(
            "path not found: {} (under workspace {})",
            scope.display(),
            workspace_root.display()
        ));
    }

    match HybridIntel::build(workspace_root, workspace_root.display().to_string()) {
        Ok((intel, _stats)) => execute_with_intel(&intel, q, &scope),
        Err(e) => ToolResult::error(format!("locate index build failed: {e}")),
    }
}

/// Locate using an existing (run-scoped) hybrid index.
pub fn execute_with_intel(intel: &HybridIntel, query: &str, scope: &Path) -> ToolResult {
    let q = query.trim();
    if q.is_empty() {
        return ToolResult::error("locate query must not be empty");
    }
    let scope = if scope.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        scope.to_path_buf()
    };

    match intel.locate_with_stats(&LocateRequest {
        query: q.to_string(),
        scope: scope.clone(),
    }) {
        Ok((candidates, stats)) => {
            let mut output = render_candidates(q, &scope, &candidates);
            output.push_str(&format!(
                "\n[locate_ms={} source_hits={}]\n",
                stats.locate_ms, stats.source_hits
            ));
            ToolResult {
                success: true,
                output: crate::tools::truncate_output(&output),
                diff: None,
                error: None,
                stop_reason: None,
            }
        }
        Err(e) => ToolResult::error(e.to_string()),
    }
}

/// Convenience for callers that already hold a snapshot (legacy tests).
pub fn locate_with_snapshot(
    query: &str,
    scope: &Path,
    workspace_root: &Path,
    snapshot: &ProjectSnapshot,
) -> LocateResult {
    let intel = SearchSnapshotIntel::new(workspace_root, snapshot);
    intel.locate(&LocateRequest {
        query: query.to_string(),
        scope: scope.to_path_buf(),
    })
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::llm::schema::parse_tool_call;
    use crate::state::step::Action;
    use crate::tools;
    use serde_json::json;
    use std::fs;

    #[test]
    fn tool_dispatch_and_schema_parse() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(
            dir.path().join("src/api.ts"),
            "export function dueDate() { return null; }\n",
        )
        .unwrap();

        let action = parse_tool_call(
            "locate",
            &json!({ "query": "dueDate", "path": "." }),
        )
        .unwrap();
        assert!(matches!(
            action,
            Action::Locate { ref query, .. } if query == "dueDate"
        ));

        let result = tools::dispatch(&action, dir.path());
        assert!(result.success, "{:?}", result.error);
        assert!(result.output.contains("dueDate"));
        assert!(result.output.contains("api.ts"));
        assert!(
            result.output.contains("exact_symbol")
                || result.output.contains("exact_token")
                || result.output.contains("lexical_hit")
                || result.output.contains("filename_match"),
            "{}",
            result.output
        );
    }

    #[test]
    fn empty_query_errors() {
        let dir = tempfile::tempdir().unwrap();
        let result = execute("  ", Path::new("."), dir.path());
        assert!(!result.success);
    }
}
