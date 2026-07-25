//! Candidate sources: path, symbol, and field-weighted BM25 lexical.
//!
//! Each source returns its own ranked list. Hybrid merges them with RRF.
//! Token pile-up sources are intentionally omitted.

use std::path::{Path, PathBuf};

use super::bm25::rank_bm25;
use super::query::normalize_query;
use super::rrf::RankedHit;
use super::traits::CandidateSource;
use super::types::{
    CandidateSignal, CandidateSourceError, RawCandidate, RepositoryIndex, SignalKind, Snippet,
};
use super::LocateRequest;

const MAX_SOURCE_HITS: usize = 40;
const MAX_SNIPPET_CHARS: usize = 160;

fn in_scope(path: &Path, scope: &Path) -> bool {
    if scope.as_os_str().is_empty() || scope == Path::new(".") {
        return true;
    }
    path.starts_with(scope)
}

fn clip(s: &str) -> String {
    let mut t = s.to_string();
    if t.len() > MAX_SNIPPET_CHARS {
        t.truncate(MAX_SNIPPET_CHARS);
        t.push('…');
    }
    t
}

/// Convert a source's RawCandidates into a unique path ranking (best score first).
pub fn rank_raw_by_path(raw: Vec<RawCandidate>, source_name: &str) -> Vec<RankedHit> {
    use std::collections::HashMap;
    let mut best: HashMap<PathBuf, (f32, Vec<Snippet>)> = HashMap::new();
    for hit in raw {
        let score: f32 = hit.signals.iter().map(|s| s.weight).sum();
        let entry = best.entry(hit.path.clone()).or_insert((0.0, Vec::new()));
        if score > entry.0 {
            entry.0 = score;
            entry.1 = hit.snippets;
        } else {
            for s in hit.snippets {
                if !entry.1.iter().any(|x| x.line == s.line && x.text == s.text) {
                    entry.1.push(s);
                }
            }
        }
    }
    let mut ranked: Vec<(PathBuf, f32, Vec<Snippet>)> = best
        .into_iter()
        .map(|(p, (s, sn))| (p, s, sn))
        .collect();
    ranked.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    ranked.truncate(MAX_SOURCE_HITS);
    ranked
        .into_iter()
        .map(|(path, _score, snippets)| RankedHit {
            path,
            snippets,
            reason: source_name.to_string(),
        })
        .collect()
}

pub struct PathCandidateSource;

impl CandidateSource for PathCandidateSource {
    fn name(&self) -> &'static str {
        "path"
    }

    fn generate(
        &self,
        request: &LocateRequest,
        index: &RepositoryIndex,
    ) -> Result<Vec<RawCandidate>, CandidateSourceError> {
        let q = normalize_query(&request.query);
        let mut out = Vec::new();
        for path in index.files.keys() {
            if !in_scope(path, &request.scope) {
                continue;
            }
            let path_str = path.to_string_lossy().replace('\\', "/");
            let path_lower = path_str.to_ascii_lowercase();
            let file_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            let stem = path
                .file_stem()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();

            let mut best = 0.0_f32;
            let mut detail = String::new();
            for v in &q.variants {
                let vl = v.to_ascii_lowercase();
                if vl.len() < 2 {
                    continue;
                }
                let score = if stem == vl {
                    10.0
                } else if file_name.starts_with(&vl) || stem.starts_with(&vl) {
                    8.0
                } else if file_name.contains(&vl) {
                    6.0
                } else if path_lower.contains(&vl) {
                    3.0
                } else {
                    0.0
                };
                if score > best {
                    best = score;
                    detail = v.clone();
                }
            }
            if best > 0.0 {
                out.push(RawCandidate {
                    path: path.clone(),
                    signals: vec![CandidateSignal {
                        kind: if best >= 6.0 {
                            SignalKind::FilenameMatch
                        } else {
                            SignalKind::PathMatch
                        },
                        weight: best,
                        detail: Some(detail),
                    }],
                    snippets: vec![],
                });
            }
        }
        Ok(out)
    }
}

pub struct SymbolCandidateSource;

impl CandidateSource for SymbolCandidateSource {
    fn name(&self) -> &'static str {
        "symbol"
    }

    fn generate(
        &self,
        request: &LocateRequest,
        index: &RepositoryIndex,
    ) -> Result<Vec<RawCandidate>, CandidateSourceError> {
        let q = normalize_query(&request.query);
        let mut out = Vec::new();
        for v in &q.variants {
            for loc in index.symbols_named(v) {
                if !in_scope(&loc.path, &request.scope) {
                    continue;
                }
                out.push(RawCandidate {
                    path: loc.path.clone(),
                    signals: vec![CandidateSignal {
                        kind: SignalKind::ExactSymbol,
                        weight: 12.0,
                        detail: Some(v.clone()),
                    }],
                    snippets: vec![Snippet {
                        line: loc.line,
                        text: format!("{} (symbol)", v),
                    }],
                });
            }
            // Partial: symbol name contains variant (weaker).
            let vl = v.to_ascii_lowercase();
            if vl.len() < 3 {
                continue;
            }
            for (name, locs) in &index.symbols_by_name {
                if name.to_ascii_lowercase().contains(&vl) && name != v {
                    for loc in locs {
                        if !in_scope(&loc.path, &request.scope) {
                            continue;
                        }
                        out.push(RawCandidate {
                            path: loc.path.clone(),
                            signals: vec![CandidateSignal {
                                kind: SignalKind::ExactSymbol,
                                weight: 4.0,
                                detail: Some(name.clone()),
                            }],
                            snippets: vec![Snippet {
                                line: loc.line,
                                text: format!("{name} (symbol~)"),
                            }],
                        });
                    }
                }
            }
        }
        Ok(out)
    }
}

/// Field-weighted BM25 over the repository index (no per-hit token accumulation).
pub struct LexicalCandidateSource {
    pub max_hits: usize,
}

impl Default for LexicalCandidateSource {
    fn default() -> Self {
        Self {
            max_hits: MAX_SOURCE_HITS,
        }
    }
}

impl LexicalCandidateSource {
    pub fn new(_workspace_root: impl Into<PathBuf>) -> Self {
        Self::default()
    }
}

impl CandidateSource for LexicalCandidateSource {
    fn name(&self) -> &'static str {
        "bm25"
    }

    fn generate(
        &self,
        request: &LocateRequest,
        index: &RepositoryIndex,
    ) -> Result<Vec<RawCandidate>, CandidateSourceError> {
        let q = normalize_query(&request.query);
        // Prefer short terms + variants for BM25 (avoid flooding with camelCase duplicates).
        let mut terms = q.terms.clone();
        for v in &q.variants {
            if v.len() >= 2 && !terms.iter().any(|t| t.eq_ignore_ascii_case(v)) {
                terms.push(v.clone());
            }
        }
        let ranked = rank_bm25(index, &terms, &request.scope, self.max_hits);
        Ok(ranked
            .into_iter()
            .map(|(path, score)| {
                let snip = index.files.get(&path).and_then(|rec| {
                    rec.symbols.first().map(|s| Snippet {
                        line: s.line,
                        text: clip(&format!("{} ({})", s.name, "symbol")),
                    })
                });
                RawCandidate {
                    path,
                    signals: vec![CandidateSignal {
                        kind: SignalKind::LexicalHit,
                        weight: score,
                        detail: Some("bm25".into()),
                    }],
                    snippets: snip.into_iter().collect(),
                }
            })
            .collect())
    }
}

/// Kept for API stability; not registered in the hybrid RRF pipeline.
pub struct TokenCandidateSource;

impl CandidateSource for TokenCandidateSource {
    fn name(&self) -> &'static str {
        "token"
    }

    fn generate(
        &self,
        request: &LocateRequest,
        index: &RepositoryIndex,
    ) -> Result<Vec<RawCandidate>, CandidateSourceError> {
        // Deprecated for hybrid ranking — empty to avoid naive token pile-up.
        let _ = (request, index);
        Ok(vec![])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo_intel::indexer::build_index;
    use std::fs;

    fn write(dir: &Path, rel: &str, body: &str) {
        let path = dir.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    #[test]
    fn symbol_and_path_sources_find_move_task() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "frontend/src/api/tasks.ts",
            "export async function moveTask() {}\n",
        );
        let (index, _) = build_index(dir.path(), "t").unwrap();
        let req = LocateRequest {
            query: "move task".into(),
            scope: PathBuf::from("."),
        };
        let sym = SymbolCandidateSource.generate(&req, &index).unwrap();
        assert!(sym.iter().any(|c| c.path.ends_with("tasks.ts")));
        let path = PathCandidateSource.generate(&req, &index).unwrap();
        assert!(path.iter().any(|c| c.path.ends_with("tasks.ts")));
    }
}
