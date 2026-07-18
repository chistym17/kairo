//! v1 candidate sources: path, token, symbol, lexical.

use std::fs;
use std::path::{Path, PathBuf};

use super::query::normalize_query;
use super::traits::CandidateSource;
use super::types::{
    CandidateSignal, CandidateSourceError, RawCandidate, RepositoryIndex, SignalKind, Snippet,
};
use super::LocateRequest;

const W_EXACT_SYMBOL: f32 = 12.0;
const W_EXACT_TOKEN: f32 = 8.0;
const W_FILENAME: f32 = 7.0;
const W_PATH: f32 = 5.0;
const W_LEXICAL: f32 = 6.0;
const MAX_LEXICAL_HITS: usize = 40;
const MAX_SNIPPET_CHARS: usize = 160;

fn in_scope(path: &Path, scope: &Path) -> bool {
    if scope.as_os_str().is_empty() || scope == Path::new(".") {
        return true;
    }
    path.starts_with(scope)
}

fn signal(kind: SignalKind, weight: f32, detail: impl Into<String>) -> CandidateSignal {
    CandidateSignal {
        kind,
        weight,
        detail: Some(detail.into()),
    }
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

            for v in &q.variants {
                let vl = v.to_ascii_lowercase();
                if file_name.contains(&vl) {
                    out.push(RawCandidate {
                        path: path.clone(),
                        signals: vec![signal(SignalKind::FilenameMatch, W_FILENAME, v.clone())],
                        snippets: vec![],
                    });
                    break;
                }
                if path_lower.contains(&vl) {
                    out.push(RawCandidate {
                        path: path.clone(),
                        signals: vec![signal(SignalKind::PathMatch, W_PATH, v.clone())],
                        snippets: vec![],
                    });
                    break;
                }
            }
        }
        Ok(out)
    }
}

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
        let q = normalize_query(&request.query);
        let mut out = Vec::new();
        for v in &q.variants {
            for path in index.files_for_token(v) {
                if !in_scope(path, &request.scope) {
                    continue;
                }
                out.push(RawCandidate {
                    path: path.clone(),
                    signals: vec![signal(SignalKind::ExactToken, W_EXACT_TOKEN, v.clone())],
                    snippets: vec![],
                });
            }
            // Case-insensitive token table probe for lowercased keys.
            let lower = v.to_ascii_lowercase();
            if lower != *v {
                for path in index.files_for_token(&lower) {
                    if !in_scope(path, &request.scope) {
                        continue;
                    }
                    out.push(RawCandidate {
                        path: path.clone(),
                        signals: vec![signal(SignalKind::ExactToken, W_EXACT_TOKEN, lower.clone())],
                        snippets: vec![],
                    });
                }
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
                    signals: vec![signal(SignalKind::ExactSymbol, W_EXACT_SYMBOL, v.clone())],
                    snippets: vec![Snippet {
                        line: loc.line,
                        text: format!("{} (symbol)", v),
                    }],
                });
            }
        }
        Ok(out)
    }
}

/// Capped content search over indexed files only.
pub struct LexicalCandidateSource {
    pub workspace_root: PathBuf,
    pub max_hits: usize,
}

impl LexicalCandidateSource {
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            max_hits: MAX_LEXICAL_HITS,
        }
    }
}

impl CandidateSource for LexicalCandidateSource {
    fn name(&self) -> &'static str {
        "lexical"
    }

    fn generate(
        &self,
        request: &LocateRequest,
        index: &RepositoryIndex,
    ) -> Result<Vec<RawCandidate>, CandidateSourceError> {
        let q = normalize_query(&request.query);
        let needles: Vec<String> = q
            .variants
            .iter()
            .map(|v| v.to_ascii_lowercase())
            .collect();
        if needles.is_empty() {
            return Ok(vec![]);
        }

        let mut out = Vec::new();
        for path in index.files.keys() {
            if out.len() >= self.max_hits {
                break;
            }
            if !in_scope(path, &request.scope) {
                continue;
            }
            let abs = self.workspace_root.join(path);
            let content = match fs::read_to_string(&abs) {
                Ok(c) => c,
                Err(_) => continue,
            };
            for (idx, line) in content.lines().enumerate() {
                if out.len() >= self.max_hits {
                    break;
                }
                let lower = line.to_ascii_lowercase();
                for needle in &needles {
                    if lower.contains(needle) {
                        let text = clip(line.trim());
                        out.push(RawCandidate {
                            path: path.clone(),
                            signals: vec![signal(SignalKind::LexicalHit, W_LEXICAL, needle.clone())],
                            snippets: vec![Snippet {
                                line: idx + 1,
                                text,
                            }],
                        });
                        break;
                    }
                }
            }
        }
        Ok(out)
    }
}

fn clip(s: &str) -> String {
    let mut t = s.to_string();
    if t.len() > MAX_SNIPPET_CHARS {
        t.truncate(MAX_SNIPPET_CHARS);
        t.push('…');
    }
    t
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
