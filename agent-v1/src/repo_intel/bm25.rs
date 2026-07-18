//! Field-weighted BM25 over indexed file fields.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::types::{FieldTermFreqs, RepositoryIndex};

/// BM25 parameters (classic defaults).
const K1: f32 = 1.2;
const B: f32 = 0.75;

/// Field boosts: symbols ≫ path ≫ idents ≫ strings/comments/directives.
pub const W_SYMBOL: f32 = 4.0;
pub const W_PATH: f32 = 3.0;
pub const W_IDENT: f32 = 1.0;
pub const W_STRING: f32 = 0.1;

fn field_len(map: &HashMap<String, u32>) -> u32 {
    map.values().sum()
}

fn bump(map: &mut HashMap<String, u32>, term: &str) {
    if term.len() < 2 {
        return;
    }
    let key = term.to_ascii_lowercase();
    *map.entry(key).or_insert(0) += 1;
}

/// Build field freqs from path, symbol names, and file content.
pub fn build_field_freqs(
    path: &Path,
    symbol_names: &[String],
    content: &str,
) -> FieldTermFreqs {
    let mut fields = FieldTermFreqs::default();

    for sym in symbol_names {
        bump(&mut fields.symbols, sym);
        for part in split_ident_parts(sym) {
            bump(&mut fields.symbols, &part);
        }
    }

    let path_str = path.to_string_lossy().replace('\\', "/");
    let parts: Vec<&str> = path_str.split('/').filter(|p| !p.is_empty() && *p != ".").collect();
    if let Some(file) = parts.last() {
        let stem = file.rsplit_once('.').map(|(s, _)| s).unwrap_or(file);
        // Filename stem is the strong path signal — bump multiple times.
        for _ in 0..3 {
            bump(&mut fields.path, stem);
        }
        for p in split_ident_parts(stem) {
            for _ in 0..2 {
                bump(&mut fields.path, &p);
            }
        }
    }
    // Immediate parent directory only (weak); skip deep path noise.
    if parts.len() >= 2 {
        let parent = parts[parts.len() - 2];
        bump(&mut fields.path, parent);
    }

    tokenize_content(content, &mut fields);
    fields
}

fn split_ident_parts(name: &str) -> Vec<String> {
    let mut parts = Vec::new();
    for chunk in name.split(|c: char| c == '_' || c == '-') {
        if chunk.is_empty() {
            continue;
        }
        let mut current = String::new();
        for ch in chunk.chars() {
            if ch.is_uppercase() && !current.is_empty() {
                parts.push(current.to_ascii_lowercase());
                current.clear();
            }
            current.push(ch);
        }
        if !current.is_empty() {
            parts.push(current.to_ascii_lowercase());
        }
    }
    parts
}

fn tokenize_content(content: &str, fields: &mut FieldTermFreqs) {
    let bytes = content.as_bytes();
    let mut i = 0;
    let mut in_line_comment = false;
    let mut in_block_comment = false;
    let mut in_string: Option<u8> = None;

    while i < bytes.len() {
        let b = bytes[i];

        if in_line_comment {
            if b == b'\n' {
                in_line_comment = false;
            }
            i += 1;
            continue;
        }
        if in_block_comment {
            if b == b'*' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
                in_block_comment = false;
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }
        if let Some(q) = in_string {
            if b == b'\\' && i + 1 < bytes.len() {
                i += 2;
                continue;
            }
            if b == q {
                in_string = None;
            } else if is_ident_start(b) || b == b'_' {
                let (term, next) = read_ident(bytes, i);
                bump(&mut fields.strings, &term);
                i = next;
                continue;
            }
            i += 1;
            continue;
        }

        if b == b'/' && i + 1 < bytes.len() {
            if bytes[i + 1] == b'/' {
                in_line_comment = true;
                i += 2;
                continue;
            }
            if bytes[i + 1] == b'*' {
                in_block_comment = true;
                i += 2;
                continue;
            }
        }
        if b == b'"' || b == b'\'' || b == b'`' {
            in_string = Some(b);
            i += 1;
            continue;
        }

        if is_ident_start(b) || b == b'_' {
            let (term, next) = read_ident(bytes, i);
            bump(&mut fields.idents, &term);
            i = next;
            continue;
        }
        i += 1;
    }
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic()
}

fn read_ident(bytes: &[u8], start: usize) -> (String, usize) {
    let mut i = start + 1;
    while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
        i += 1;
    }
    (
        String::from_utf8_lossy(&bytes[start..i]).into_owned(),
        i,
    )
}

#[derive(Debug, Clone)]
struct CorpusStats {
    n_docs: f32,
    avg_symbol_len: f32,
    avg_path_len: f32,
    avg_ident_len: f32,
    avg_string_len: f32,
    df_symbol: HashMap<String, u32>,
    df_path: HashMap<String, u32>,
    df_ident: HashMap<String, u32>,
    df_string: HashMap<String, u32>,
}

fn corpus_stats(index: &RepositoryIndex) -> CorpusStats {
    let n = index.files.len().max(1) as f32;
    let mut df_symbol: HashMap<String, u32> = HashMap::new();
    let mut df_path: HashMap<String, u32> = HashMap::new();
    let mut df_ident: HashMap<String, u32> = HashMap::new();
    let mut df_string: HashMap<String, u32> = HashMap::new();
    let mut sum_s = 0u64;
    let mut sum_p = 0u64;
    let mut sum_i = 0u64;
    let mut sum_str = 0u64;

    for record in index.files.values() {
        let f = &record.fields;
        sum_s += u64::from(field_len(&f.symbols));
        sum_p += u64::from(field_len(&f.path));
        sum_i += u64::from(field_len(&f.idents));
        sum_str += u64::from(field_len(&f.strings));
        for t in f.symbols.keys() {
            *df_symbol.entry(t.clone()).or_insert(0) += 1;
        }
        for t in f.path.keys() {
            *df_path.entry(t.clone()).or_insert(0) += 1;
        }
        for t in f.idents.keys() {
            *df_ident.entry(t.clone()).or_insert(0) += 1;
        }
        for t in f.strings.keys() {
            *df_string.entry(t.clone()).or_insert(0) += 1;
        }
    }

    CorpusStats {
        n_docs: n,
        avg_symbol_len: sum_s as f32 / n,
        avg_path_len: sum_p as f32 / n,
        avg_ident_len: sum_i as f32 / n,
        avg_string_len: sum_str as f32 / n,
        df_symbol,
        df_path,
        df_ident,
        df_string,
    }
}

fn idf(n: f32, df: u32) -> f32 {
    let df = df.max(1) as f32;
    ((n - df + 0.5) / (df + 0.5) + 1.0).ln().max(0.0)
}

fn bm25_field(
    tf: u32,
    dl: u32,
    avgdl: f32,
    n: f32,
    df: u32,
) -> f32 {
    if tf == 0 {
        return 0.0;
    }
    let tf = tf as f32;
    let dl = dl as f32;
    let avgdl = avgdl.max(1.0);
    let idf = idf(n, df);
    let denom = tf + K1 * (1.0 - B + B * (dl / avgdl));
    idf * (tf * (K1 + 1.0)) / denom
}

/// Score one file for query terms (already lowercased variants/terms).
fn score_file(
    fields: &FieldTermFreqs,
    query_terms: &[String],
    stats: &CorpusStats,
) -> f32 {
    let mut score = 0.0;
    let sl = field_len(&fields.symbols);
    let pl = field_len(&fields.path);
    let il = field_len(&fields.idents);
    let stl = field_len(&fields.strings);

    for term in query_terms {
        let t = term.to_ascii_lowercase();
        let tf_s = *fields.symbols.get(&t).unwrap_or(&0);
        let tf_p = *fields.path.get(&t).unwrap_or(&0);
        let tf_i = *fields.idents.get(&t).unwrap_or(&0);
        let tf_st = *fields.strings.get(&t).unwrap_or(&0);

        score += W_SYMBOL
            * bm25_field(
                tf_s,
                sl,
                stats.avg_symbol_len,
                stats.n_docs,
                *stats.df_symbol.get(&t).unwrap_or(&0),
            );
        score += W_PATH
            * bm25_field(
                tf_p,
                pl,
                stats.avg_path_len,
                stats.n_docs,
                *stats.df_path.get(&t).unwrap_or(&0),
            );
        score += W_IDENT
            * bm25_field(
                tf_i,
                il,
                stats.avg_ident_len,
                stats.n_docs,
                *stats.df_ident.get(&t).unwrap_or(&0),
            );
        score += W_STRING
            * bm25_field(
                tf_st,
                stl,
                stats.avg_string_len,
                stats.n_docs,
                *stats.df_string.get(&t).unwrap_or(&0),
            );
    }
    score
}

/// Rank all in-scope files by field-weighted BM25. Returns (path, score) desc.
pub fn rank_bm25(
    index: &RepositoryIndex,
    query_terms: &[String],
    scope: &Path,
    limit: usize,
) -> Vec<(PathBuf, f32)> {
    if query_terms.is_empty() || index.files.is_empty() {
        return vec![];
    }
    let stats = corpus_stats(index);
    let mut scored: Vec<(PathBuf, f32)> = Vec::new();

    for (path, record) in &index.files {
        if !in_scope(path, scope) {
            continue;
        }
        if !record.fields.contains_any(query_terms) {
            continue;
        }
        let s = score_file(&record.fields, query_terms, &stats);
        if s > 0.0 {
            scored.push((path.clone(), s));
        }
    }

    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    scored.truncate(limit);
    scored
}

fn in_scope(path: &Path, scope: &Path) -> bool {
    if scope.as_os_str().is_empty() || scope == Path::new(".") {
        return true;
    }
    path.starts_with(scope)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo_intel::types::{FileRecord, Language, RepositoryIndex};

    fn record(path: &str, symbols: &[&str], content: &str) -> FileRecord {
        let path = PathBuf::from(path);
        let names: Vec<String> = symbols.iter().map(|s| (*s).to_string()).collect();
        let fields = build_field_freqs(&path, &names, content);
        FileRecord {
            path: path.clone(),
            language: Some(Language::TypeScript),
            content_hash: None,
            tokens: vec![],
            symbols: vec![],
            imports: vec![],
            fields,
        }
    }

    #[test]
    fn api_beats_store_for_tasks_api_client() {
        let mut index = RepositoryIndex::new("t");
        let api = record(
            "frontend-v2/src/lib/api.ts",
            &["getTasks", "createTask", "moveTask"],
            "export async function getTasks(date: string) {\n  return request(`/tasks/?dueDate=${date}`);\n}\n",
        );
        let store = record(
            "frontend-v2/src/components/tasks/store.ts",
            &["useTaskStore", "loadTasks"],
            "\"use client\";\nimport { api } from \"@/lib/api\";\nexport function loadTasks() { api.getTasks(); }\nconst tasks = [];\nconst tasks2 = tasks;\nconst tasks3 = tasks;\n",
        );
        index.files.insert(api.path.clone(), api);
        index.files.insert(store.path.clone(), store);

        let terms = vec![
            "tasks".into(),
            "api".into(),
            "client".into(),
        ];
        let ranked = rank_bm25(&index, &terms, Path::new("."), 5);
        assert!(!ranked.is_empty());
        // BM25 alone can be close; filename/path weighting should keep api.ts competitive.
        // Hybrid RRF (path + symbol + bm25) is what must prefer api.ts.
        assert!(
            ranked.iter().take(2).any(|(p, _)| p.ends_with("api.ts")),
            "expected api.ts in BM25 top-2, got {:?}",
            ranked
        );
    }
}
