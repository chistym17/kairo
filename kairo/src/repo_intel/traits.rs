//! Extension seams for hybrid repository localization.
//!
//! Public behavior lives on [`RepositoryIntelligence`] (see parent module).
//! New capabilities should plug in here as indexers, sources, rankers, etc. —
//! not by rewriting the agent loop.

use std::path::{Path, PathBuf};

use super::types::{
    CandidateSignal, CandidateSourceError, FileChange, FileRecord, Language, LocateCandidate,
    LocateError, MergedCandidate, RawCandidate, RepositoryIndex, RepositoryIntelligenceError,
    Snippet, SymbolRecord,
};
use super::LocateRequest;

/// Index one file into a [`FileRecord`] (symbols, tokens, imports, …).
pub trait FileIndexer: Send + Sync {
    fn index_file(&self, path: &Path, content: &str) -> FileRecord;
}

/// Language-limited symbol extraction (regex today, Tree-sitter later).
pub trait SymbolExtractor: Send + Sync {
    fn supports(&self, language: &Language) -> bool;

    fn extract(&self, path: &Path, content: &str) -> Vec<SymbolRecord>;
}

/// Produce raw candidates for a locate query (main retrieval seam).
pub trait CandidateSource: Send + Sync {
    fn name(&self) -> &'static str;

    fn generate(
        &self,
        request: &LocateRequest,
        index: &RepositoryIndex,
    ) -> Result<Vec<RawCandidate>, CandidateSourceError>;
}

/// Merge raw hits that share a path into one candidate with combined signals.
pub trait CandidateMerger: Send + Sync {
    fn merge(&self, raw: Vec<RawCandidate>) -> Vec<MergedCandidate>;
}

/// Rank merged candidates from observable signals.
pub trait CandidateRanker: Send + Sync {
    fn rank(&self, candidates: Vec<MergedCandidate>, limit: usize) -> Vec<LocateCandidate>;
}

/// Attach compact snippets to ranked (or merged) candidates.
pub trait SnippetProvider: Send + Sync {
    fn snippet_for(&self, path: &Path, content: &str, prefer_line: Option<usize>) -> Option<Snippet>;
}

/// Optional future: persistent / content-hash backed storage.
pub trait IndexStorage: Send + Sync {
    fn load(&self) -> Result<Option<RepositoryIndex>, RepositoryIntelligenceError>;
    fn store(&self, index: &RepositoryIndex) -> Result<(), RepositoryIntelligenceError>;
}

/// Hybrid public surface (additive `update`; locate keeps current tool shape for now).
///
/// Deliberate versioning is allowed later (async index, cancellation, …).
/// Retrieval backends must not force agent-loop changes.
pub trait HybridRepositoryIntelligence: Send + Sync {
    fn locate_ranked(
        &self,
        request: &LocateRequest,
    ) -> Result<Vec<LocateCandidate>, LocateError>;

    fn update(&mut self, changes: &[FileChange]) -> Result<(), RepositoryIntelligenceError>;

    fn index(&self) -> &RepositoryIndex;
}

/// Default merge: group by path, concatenate signals/snippets (dedup snippet text).
pub struct DefaultCandidateMerger;

impl CandidateMerger for DefaultCandidateMerger {
    fn merge(&self, raw: Vec<RawCandidate>) -> Vec<MergedCandidate> {
        use std::collections::BTreeMap;

        let mut map: BTreeMap<PathBuf, MergedCandidate> = BTreeMap::new();
        for hit in raw {
            let entry = map.entry(hit.path.clone()).or_insert(MergedCandidate {
                path: hit.path,
                signals: Vec::new(),
                snippets: Vec::new(),
            });
            entry.signals.extend(hit.signals);
            for snip in hit.snippets {
                if !entry
                    .snippets
                    .iter()
                    .any(|s| s.line == snip.line && s.text == snip.text)
                {
                    entry.snippets.push(snip);
                }
            }
        }
        map.into_values().collect()
    }
}

/// Simple deterministic ranker: sum signal weights; reasons from signal kinds.
pub struct SignalSumRanker;

impl CandidateRanker for SignalSumRanker {
    fn rank(&self, mut candidates: Vec<MergedCandidate>, limit: usize) -> Vec<LocateCandidate> {
        let mut ranked: Vec<LocateCandidate> = candidates
            .drain(..)
            .map(|c| {
                let score: f32 = c.signals.iter().map(|s| s.weight).sum();
                let reasons = reasons_from_signals(&c.signals);
                LocateCandidate {
                    path: c.path,
                    score,
                    reasons,
                    snippets: c.snippets,
                }
            })
            .collect();

        ranked.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.path.cmp(&b.path))
        });
        ranked.truncate(limit);
        ranked
    }
}

fn reasons_from_signals(signals: &[CandidateSignal]) -> Vec<String> {
    use super::types::SignalKind;
    let mut out = Vec::new();
    for s in signals {
        let label = match s.kind {
            SignalKind::ExactSymbol => "exact_symbol",
            SignalKind::ExactToken => "exact_token",
            SignalKind::FilenameMatch => "filename_match",
            SignalKind::PathMatch => "path_match",
            SignalKind::ImportNeighbor => "import_neighbor",
            SignalKind::DefinitionLike => "definition_like",
            SignalKind::LexicalHit => "lexical_hit",
        };
        let reason = match &s.detail {
            Some(d) => format!("{label}:{d}"),
            None => label.to_string(),
        };
        if !out.contains(&reason) {
            out.push(reason);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo_intel::types::{CandidateSignal, SignalKind, Snippet};
    use std::path::PathBuf;

    struct StubSource;

    impl CandidateSource for StubSource {
        fn name(&self) -> &'static str {
            "stub"
        }

        fn generate(
            &self,
            _request: &LocateRequest,
            _index: &RepositoryIndex,
        ) -> Result<Vec<RawCandidate>, CandidateSourceError> {
            Ok(vec![
                RawCandidate {
                    path: PathBuf::from("a.ts"),
                    signals: vec![CandidateSignal {
                        kind: SignalKind::FilenameMatch,
                        weight: 7.0,
                        detail: None,
                    }],
                    snippets: vec![],
                },
                RawCandidate {
                    path: PathBuf::from("a.ts"),
                    signals: vec![CandidateSignal {
                        kind: SignalKind::ExactSymbol,
                        weight: 12.0,
                        detail: Some("foo".into()),
                    }],
                    snippets: vec![Snippet {
                        line: 3,
                        text: "function foo() {}".into(),
                    }],
                },
            ])
        }
    }

    #[test]
    fn merger_and_ranker_pipeline() {
        let index = RepositoryIndex::new("test");
        let req = LocateRequest {
            query: "foo".into(),
            scope: PathBuf::from("."),
        };
        let raw = StubSource.generate(&req, &index).unwrap();
        let merged = DefaultCandidateMerger.merge(raw);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].signals.len(), 2);

        let ranked = SignalSumRanker.rank(merged, 5);
        assert_eq!(ranked.len(), 1);
        assert!((ranked[0].score - 19.0).abs() < f32::EPSILON);
        assert!(ranked[0].reasons.iter().any(|r| r.contains("exact_symbol")));
    }
}
