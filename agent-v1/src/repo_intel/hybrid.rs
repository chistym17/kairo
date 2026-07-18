//! Hybrid repository intelligence: sources → merge → rank → expand → rerank.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Instant;

use super::expand::expand_imports;
use super::indexer::{apply_changes, build_index, IndexBuildStats, IndexUpdateStats};
use super::snippets::LineSnippetProvider;
use super::sources::{
    LexicalCandidateSource, PathCandidateSource, SymbolCandidateSource, TokenCandidateSource,
};
use super::traits::{
    CandidateMerger, CandidateRanker, CandidateSource, DefaultCandidateMerger,
    HybridRepositoryIntelligence, SignalSumRanker,
};
use super::types::{
    FileChange, LocateCandidate, LocateError, RawCandidate, RepositoryIndex,
    RepositoryIntelligenceError,
};
use super::LocateRequest;

pub const HYBRID_TOP_K: usize = 5;
pub const IMPORT_EXPAND_TOP_N: usize = 3;

#[derive(Debug, Clone, Default)]
pub struct LocateStats {
    pub locate_ms: u128,
    pub source_hits: usize,
    pub result_count: usize,
}

pub struct HybridIntel {
    workspace_root: PathBuf,
    index: RepositoryIndex,
    sources: Vec<Box<dyn CandidateSource>>,
    merger: DefaultCandidateMerger,
    ranker: SignalSumRanker,
    snippets: LineSnippetProvider,
}

impl fmt::Debug for HybridIntel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HybridIntel")
            .field("workspace_root", &self.workspace_root)
            .field("files", &self.index.file_count())
            .field("revision", &self.index.revision)
            .field("sources", &self.sources.len())
            .finish()
    }
}

impl HybridIntel {
    pub fn build(
        workspace_root: &Path,
        workspace_id: impl Into<String>,
    ) -> Result<(Self, IndexBuildStats), RepositoryIntelligenceError> {
        let (index, stats) = build_index(workspace_root, workspace_id)?;
        Ok((Self::from_index(workspace_root, index), stats))
    }

    pub fn from_index(workspace_root: &Path, index: RepositoryIndex) -> Self {
        let sources: Vec<Box<dyn CandidateSource>> = vec![
            Box::new(PathCandidateSource),
            Box::new(TokenCandidateSource),
            Box::new(SymbolCandidateSource),
            Box::new(LexicalCandidateSource::new(workspace_root)),
        ];
        Self {
            workspace_root: workspace_root.to_path_buf(),
            index,
            sources,
            merger: DefaultCandidateMerger,
            ranker: SignalSumRanker,
            snippets: LineSnippetProvider::new(workspace_root),
        }
    }

    pub fn locate_with_stats(
        &self,
        request: &LocateRequest,
    ) -> Result<(Vec<LocateCandidate>, LocateStats), LocateError> {
        let started = Instant::now();
        if request.query.trim().is_empty() {
            return Err(LocateError::EmptyQuery);
        }

        let (mut results, source_hits) = self.locate_pipeline(request)?;
        self.snippets.attach(&mut results, 2);

        let stats = LocateStats {
            locate_ms: started.elapsed().as_millis(),
            source_hits,
            result_count: results.len(),
        };
        Ok((results, stats))
    }

    fn locate_pipeline(
        &self,
        request: &LocateRequest,
    ) -> Result<(Vec<LocateCandidate>, usize), LocateError> {
        let mut raw = Vec::new();
        for source in &self.sources {
            let mut hits = source
                .generate(request, &self.index)
                .map_err(|e| LocateError::Internal(e.to_string()))?;
            raw.append(&mut hits);
        }
        let source_hits = raw.len();

        let merged = self.merger.merge(raw);
        let preliminary = self.ranker.rank(merged.clone(), 8);
        let seed_paths: Vec<PathBuf> = preliminary.iter().map(|c| c.path.clone()).collect();
        let expanded = expand_imports(&self.index, &seed_paths, IMPORT_EXPAND_TOP_N);

        let mut re_raw: Vec<RawCandidate> = merged
            .into_iter()
            .map(|m| RawCandidate {
                path: m.path,
                signals: m.signals,
                snippets: m.snippets,
            })
            .collect();
        re_raw.extend(expanded.into_iter().map(|m| RawCandidate {
            path: m.path,
            signals: m.signals,
            snippets: m.snippets,
        }));

        let re_merged = self.merger.merge(re_raw);
        let ranked = self.ranker.rank(re_merged, HYBRID_TOP_K);
        Ok((ranked, source_hits))
    }

    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    pub fn update_with_stats(
        &mut self,
        changes: &[FileChange],
    ) -> Result<IndexUpdateStats, RepositoryIntelligenceError> {
        apply_changes(&mut self.index, &self.workspace_root, changes)
    }
}

impl HybridRepositoryIntelligence for HybridIntel {
    fn locate_ranked(
        &self,
        request: &LocateRequest,
    ) -> Result<Vec<LocateCandidate>, LocateError> {
        let (results, _) = self.locate_with_stats(request)?;
        Ok(results)
    }

    fn update(&mut self, changes: &[FileChange]) -> Result<(), RepositoryIntelligenceError> {
        self.update_with_stats(changes).map(|_| ())
    }

    fn index(&self) -> &RepositoryIndex {
        &self.index
    }
}

/// Render hybrid results similarly to the legacy locate tool.
pub fn render_candidates(query: &str, scope: &Path, candidates: &[LocateCandidate]) -> String {
    let mut out = format!(
        "locate query={:?} under '{}' — {} candidate(s)\n",
        query,
        scope.display(),
        candidates.len()
    );
    if candidates.is_empty() {
        out.push_str("(no candidates)\n");
        return out;
    }
    for (i, c) in candidates.iter().enumerate() {
        out.push_str(&format!(
            "\n{}. {}  score={:.1}\n",
            i + 1,
            c.path.display(),
            c.score
        ));
        if !c.reasons.is_empty() {
            out.push_str("   reasons: ");
            out.push_str(&c.reasons.join(", "));
            out.push('\n');
        }
        for snip in &c.snippets {
            out.push_str(&format!("   L{}: {}\n", snip.line, snip.text));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(dir: &Path, rel: &str, body: &str) {
        let path = dir.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    #[test]
    fn hybrid_locate_ranks_move_task_file() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "frontend/src/api/tasks.ts",
            "import { helper } from './helper';\nexport async function moveTask(id) { return id; }\n",
        );
        write(
            dir.path(),
            "frontend/src/api/helper.ts",
            "export function helper() { return 1; }\n",
        );
        write(
            dir.path(),
            "frontend/src/components/Board.tsx",
            "export function Board() { return null; }\n",
        );

        let (intel, _) = HybridIntel::build(dir.path(), "t").unwrap();
        let results = intel
            .locate_ranked(&LocateRequest {
                query: "move task due date".into(),
                scope: PathBuf::from("."),
            })
            .unwrap();

        assert!(!results.is_empty());
        assert!(
            results[0].path.ends_with("tasks.ts"),
            "top={:?}",
            results[0]
        );
        assert!(
            results.iter().any(|c| c.path.ends_with("helper.ts")),
            "{:?}",
            results
        );
    }
}
