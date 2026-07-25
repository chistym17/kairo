//! Build and incrementally update [`RepositoryIndex`].

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::tools::ignore::is_ignored_dir_name;

use super::extractors::CompositeSymbolExtractor;
use super::traits::FileIndexer;
use super::types::{
    FileChange, FileChangeKind, FileHash, FileRecord, ImportReference, Language, RepositoryIndex,
    RepositoryIntelligenceError, SymbolLocation, SymbolRecord,
};
use super::bm25::build_field_freqs;

const MAX_FILE_BYTES: u64 = 512 * 1024;
const MAX_INDEX_FILES: usize = 8_000;
const MAX_TOKENS_PER_FILE: usize = 400;

#[derive(Debug, Clone, Default)]
pub struct IndexBuildStats {
    pub files_indexed: usize,
    pub files_skipped: usize,
    pub truncated: bool,
    pub build_ms: u128,
}

#[derive(Debug, Clone, Default)]
pub struct IndexUpdateStats {
    pub files_updated: usize,
    pub files_removed: usize,
    pub update_ms: u128,
}

/// Default indexer: tokens + regex symbols + best-effort local imports.
pub struct DefaultFileIndexer {
    workspace_root: PathBuf,
    symbols: CompositeSymbolExtractor,
}

impl DefaultFileIndexer {
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            symbols: CompositeSymbolExtractor::default(),
        }
    }
}

impl FileIndexer for DefaultFileIndexer {
    fn index_file(&self, path: &Path, content: &str) -> FileRecord {
        let language = Some(Language::from_path(path));
        let content_hash = Some(fnv1a_hash(content.as_bytes()));
        let symbols = self.symbols.extract_for_path(path, content);
        let imports = extract_imports(path, content, &self.workspace_root);
        let tokens = collect_tokens(path, content, &symbols);
        let symbol_names: Vec<String> = symbols.iter().map(|s| s.name.clone()).collect();
        let fields = build_field_freqs(path, &symbol_names, content);

        FileRecord {
            path: path.to_path_buf(),
            language,
            content_hash,
            tokens,
            symbols,
            imports,
            fields,
        }
    }
}

/// Scan workspace and build a fresh index.
pub fn build_index(
    workspace_root: &Path,
    workspace_id: impl Into<String>,
) -> Result<(RepositoryIndex, IndexBuildStats), RepositoryIntelligenceError> {
    let indexer = DefaultFileIndexer::new(workspace_root);
    build_index_with(workspace_root, workspace_id, &indexer)
}

pub fn build_index_with(
    workspace_root: &Path,
    workspace_id: impl Into<String>,
    indexer: &dyn FileIndexer,
) -> Result<(RepositoryIndex, IndexBuildStats), RepositoryIntelligenceError> {
    let started = Instant::now();
    let mut index = RepositoryIndex::new(workspace_id);
    if let Some(oid) = read_git_head(workspace_root) {
        index.revision.git_head = Some(oid);
    }

    let mut stats = IndexBuildStats::default();
    let mut paths = Vec::new();
    collect_source_files(workspace_root, workspace_root, &mut paths, &mut stats)?;

    for abs in paths {
        let rel = abs
            .strip_prefix(workspace_root)
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|_| abs.clone());
        match index_abs_file(&rel, &abs, indexer) {
            Ok(Some(record)) => {
                index.files.insert(rel, record);
                stats.files_indexed += 1;
            }
            Ok(None) => stats.files_skipped += 1,
            Err(_) => stats.files_skipped += 1,
        }
    }

    rebuild_lookups(&mut index);
    stats.build_ms = started.elapsed().as_millis();
    Ok((index, stats))
}

/// Apply file changes; only touched paths are reindexed.
pub fn apply_changes(
    index: &mut RepositoryIndex,
    workspace_root: &Path,
    changes: &[FileChange],
) -> Result<IndexUpdateStats, RepositoryIntelligenceError> {
    let indexer = DefaultFileIndexer::new(workspace_root);
    apply_changes_with(index, workspace_root, changes, &indexer)
}

pub fn apply_changes_with(
    index: &mut RepositoryIndex,
    workspace_root: &Path,
    changes: &[FileChange],
    indexer: &dyn FileIndexer,
) -> Result<IndexUpdateStats, RepositoryIntelligenceError> {
    let started = Instant::now();
    let mut stats = IndexUpdateStats::default();

    for change in changes {
        let rel = normalize_rel(&change.path);
        match change.kind {
            FileChangeKind::Deleted => {
                if index.files.remove(&rel).is_some() {
                    stats.files_removed += 1;
                }
            }
            FileChangeKind::Created | FileChangeKind::Modified => {
                let abs = workspace_root.join(&rel);
                if !abs.exists() {
                    if index.files.remove(&rel).is_some() {
                        stats.files_removed += 1;
                    }
                    continue;
                }
                match index_abs_file(&rel, &abs, indexer) {
                    Ok(Some(record)) => {
                        index.files.insert(rel, record);
                        stats.files_updated += 1;
                    }
                    Ok(None) => {
                        index.files.remove(&rel);
                    }
                    Err(e) => return Err(e),
                }
            }
        }
    }

    if stats.files_updated > 0 || stats.files_removed > 0 {
        index.revision.bump_dirty();
        if let Some(oid) = read_git_head(workspace_root) {
            index.revision.git_head = Some(oid);
        }
        rebuild_lookups(index);
    }

    stats.update_ms = started.elapsed().as_millis();
    Ok(stats)
}

fn index_abs_file(
    rel: &Path,
    abs: &Path,
    indexer: &dyn FileIndexer,
) -> Result<Option<FileRecord>, RepositoryIntelligenceError> {
    let meta = fs::metadata(abs).map_err(|e| {
        RepositoryIntelligenceError::UpdateFailed(format!("{}: {e}", abs.display()))
    })?;
    if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
        return Ok(None);
    }
    if !is_indexable_path(rel) {
        return Ok(None);
    }
    let content = fs::read_to_string(abs).map_err(|e| {
        RepositoryIntelligenceError::UpdateFailed(format!("read {}: {e}", abs.display()))
    })?;
    Ok(Some(indexer.index_file(rel, &content)))
}

fn collect_source_files(
    workspace_root: &Path,
    cur: &Path,
    out: &mut Vec<PathBuf>,
    stats: &mut IndexBuildStats,
) -> Result<(), RepositoryIntelligenceError> {
    if out.len() >= MAX_INDEX_FILES {
        stats.truncated = true;
        return Ok(());
    }
    let entries = fs::read_dir(cur).map_err(|e| {
        RepositoryIntelligenceError::Internal(format!("read_dir {}: {e}", cur.display()))
    })?;
    for entry in entries.flatten() {
        if out.len() >= MAX_INDEX_FILES {
            stats.truncated = true;
            break;
        }
        let path = entry.path();
        if path.is_dir() {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if is_ignored_dir_name(name) {
                    continue;
                }
            }
            collect_source_files(workspace_root, &path, out, stats)?;
        } else if path.is_file() {
            let rel = path
                .strip_prefix(workspace_root)
                .unwrap_or(path.as_path());
            if is_indexable_path(rel) {
                out.push(path);
            } else {
                stats.files_skipped += 1;
            }
        }
    }
    Ok(())
}

fn is_indexable_path(rel: &Path) -> bool {
    matches!(
        Language::from_path(rel),
        Language::Rust | Language::Python | Language::JavaScript | Language::TypeScript
    )
}

fn normalize_rel(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        use std::path::Component;
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(s) => out.push(s),
            Component::RootDir | Component::Prefix(_) => {}
        }
    }
    if out.as_os_str().is_empty() {
        path.to_path_buf()
    } else {
        out
    }
}

pub fn rebuild_lookups(index: &mut RepositoryIndex) {
    index.symbols_by_name.clear();
    index.files_by_token.clear();
    index.imports_by_file.clear();

    for (path, record) in &index.files {
        for sym in &record.symbols {
            index
                .symbols_by_name
                .entry(sym.name.clone())
                .or_default()
                .push(SymbolLocation {
                    path: path.clone(),
                    kind: sym.kind,
                    line: sym.line,
                });
        }
        for token in &record.tokens {
            let entry = index.files_by_token.entry(token.clone()).or_default();
            if !entry.iter().any(|p| p == path) {
                entry.push(path.clone());
            }
        }
        if !record.imports.is_empty() {
            index
                .imports_by_file
                .insert(path.clone(), record.imports.clone());
        }
    }
}

fn collect_tokens(path: &Path, content: &str, symbols: &[SymbolRecord]) -> Vec<String> {
    let mut tokens = Vec::new();
    let push = |tokens: &mut Vec<String>, s: String| {
        if tokens.len() >= MAX_TOKENS_PER_FILE {
            return;
        }
        if s.len() < 2 || s.len() > 64 {
            return;
        }
        if !tokens.iter().any(|t| t == &s) {
            tokens.push(s);
        }
    };

    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
        for part in split_ident(stem) {
            push(&mut tokens, part);
        }
        push(&mut tokens, stem.to_string());
    }
    for sym in symbols {
        push(&mut tokens, sym.name.clone());
        for part in split_ident(&sym.name) {
            push(&mut tokens, part);
        }
    }

    for word in content.split(|c: char| !c.is_ascii_alphanumeric() && c != '_') {
        if tokens.len() >= MAX_TOKENS_PER_FILE {
            break;
        }
        if word.len() >= 3 && word.bytes().any(|b| b.is_ascii_alphabetic()) {
            push(&mut tokens, word.to_string());
        }
    }
    tokens
}

fn split_ident(name: &str) -> Vec<String> {
    let mut parts = Vec::new();
    for chunk in name.split('_') {
        if chunk.is_empty() {
            continue;
        }
        let mut current = String::new();
        for ch in chunk.chars() {
            if ch.is_uppercase() && !current.is_empty() {
                parts.push(current.clone());
                current.clear();
            }
            current.push(ch.to_ascii_lowercase());
        }
        if !current.is_empty() {
            parts.push(current);
        }
    }
    parts
}

fn extract_imports(path: &Path, content: &str, workspace_root: &Path) -> Vec<ImportReference> {
    let lang = Language::from_path(path);
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    let mut out = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();
        let raw = match lang {
            Language::JavaScript | Language::TypeScript => js_import_raw(trimmed),
            Language::Python => py_import_raw(trimmed),
            Language::Rust => rust_import_raw(trimmed),
            Language::Other => None,
        };
        let Some(raw) = raw else { continue };
        if out.iter().any(|i: &ImportReference| i.raw == raw) {
            continue;
        }
        let resolved = resolve_local_import(parent, &raw, lang, workspace_root);
        out.push(ImportReference { raw, resolved });
    }
    out
}

fn js_import_raw(trimmed: &str) -> Option<String> {
    if let Some(idx) = trimmed.find("from ") {
        let rest = trimmed[idx + 5..].trim_start();
        return take_quoted(rest);
    }
    if let Some(rest) = trimmed.strip_prefix("require(") {
        return take_quoted(rest);
    }
    None
}

fn py_import_raw(trimmed: &str) -> Option<String> {
    if let Some(rest) = trimmed.strip_prefix("from ") {
        let mod_path = rest.split_whitespace().next()?;
        if mod_path.starts_with('.') {
            return Some(mod_path.to_string());
        }
        return None;
    }
    None
}

fn rust_import_raw(trimmed: &str) -> Option<String> {
    let rest = trimmed.strip_prefix("use ")?;
    let path = rest
        .split(|c: char| c == ';' || c == '{' || c == ' ')
        .next()?
        .trim();
    if path.starts_with("crate::") || path.starts_with("super::") || path.starts_with("self::") {
        return Some(path.trim_end_matches("::").to_string());
    }
    None
}

fn take_quoted(s: &str) -> Option<String> {
    let s = s.trim_start();
    let quote = s.chars().next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }
    let rest = &s[quote.len_utf8()..];
    let end = rest.find(quote)?;
    Some(rest[..end].to_string())
}

fn resolve_local_import(
    parent: &Path,
    raw: &str,
    lang: Language,
    workspace_root: &Path,
) -> Option<PathBuf> {
    match lang {
        Language::JavaScript | Language::TypeScript => {
            if !(raw.starts_with("./") || raw.starts_with("../")) {
                return None;
            }
            let base = normalize_rel(&parent.join(raw));
            let candidates = js_candidates(&base);
            for cand in &candidates {
                if workspace_root.join(cand).is_file() {
                    return Some(cand.clone());
                }
            }
            // Truthful best-effort: first plausible relative path even if missing.
            candidates.into_iter().next()
        }
        Language::Python => {
            if !raw.starts_with('.') {
                return None;
            }
            let dots = raw.chars().take_while(|c| *c == '.').count();
            let rest = &raw[dots..];
            let mut dir = parent.to_path_buf();
            for _ in 1..dots {
                if !dir.pop() {
                    break;
                }
            }
            if rest.is_empty() {
                let init = normalize_rel(&dir.join("__init__.py"));
                return Some(init);
            }
            let rel = rest.replace('.', "/");
            let file = normalize_rel(&dir.join(format!("{rel}.py")));
            let pkg = normalize_rel(&dir.join(&rel).join("__init__.py"));
            if workspace_root.join(&file).is_file() {
                Some(file)
            } else if workspace_root.join(&pkg).is_file() {
                Some(pkg)
            } else {
                Some(file)
            }
        }
        Language::Rust | Language::Other => None,
    }
}

fn js_candidates(base: &Path) -> Vec<PathBuf> {
    if base.extension().is_some() {
        return vec![base.to_path_buf()];
    }
    let mut out = Vec::new();
    for ext in ["ts", "tsx", "js", "jsx", "mjs"] {
        out.push(base.with_extension(ext));
    }
    out.push(base.join("index.ts"));
    out.push(base.join("index.js"));
    out
}

fn read_git_head(workspace_root: &Path) -> Option<String> {
    let head = workspace_root.join(".git/HEAD");
    let contents = fs::read_to_string(head).ok()?;
    let contents = contents.trim();
    if let Some(rest) = contents.strip_prefix("ref: ") {
        let refer = workspace_root.join(".git").join(rest);
        fs::read_to_string(refer)
            .ok()
            .map(|s| s.trim().to_string())
    } else if contents.len() >= 7 {
        Some(contents.to_string())
    } else {
        None
    }
}

fn fnv1a_hash(bytes: &[u8]) -> FileHash {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    FileHash(format!("{hash:016x}"))
}

impl RepositoryIndex {
    pub fn files_for_token(&self, token: &str) -> &[PathBuf] {
        self.files_by_token
            .get(token)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    pub fn symbols_named(&self, name: &str) -> &[SymbolLocation] {
        self.symbols_by_name
            .get(name)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    pub fn imports_of(&self, path: &Path) -> &[ImportReference] {
        self.imports_by_file
            .get(path)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(dir: &Path, rel: &str, body: &str) {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, body).unwrap();
    }

    #[test]
    fn builds_symbols_tokens_and_imports() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "frontend/src/api/tasks.ts",
            "import { helper } from './helper';\nexport async function moveTask() {}\n",
        );
        write(
            dir.path(),
            "frontend/src/api/helper.ts",
            "export const helper = 1;\n",
        );
        write(dir.path(), "backend/tasks/views.py", "def move_task():\n    pass\n");
        write(dir.path(), "node_modules/skip.js", "export function nope() {}\n");

        let (index, stats) = build_index(dir.path(), "test").unwrap();
        assert!(stats.files_indexed >= 3);
        assert!(index.symbols_named("moveTask").len() >= 1);
        assert!(index.symbols_named("move_task").len() >= 1);
        assert!(!index.files.contains_key(Path::new("node_modules/skip.js")));

        let api = Path::new("frontend/src/api/tasks.ts");
        let imports = index.imports_of(api);
        assert!(imports.iter().any(|i| i.raw.contains("helper")));
        assert!(
            imports.iter().any(|i| {
                i.resolved
                    .as_ref()
                    .is_some_and(|p| p.ends_with("helper.ts"))
            }),
            "resolved={:?}",
            imports
        );
        assert!(index.files_for_token("moveTask").contains(&api.to_path_buf()));
    }

    #[test]
    fn incremental_update_reindexes_one_file() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "src/a.ts", "export function alpha() {}\n");
        let (mut index, _) = build_index(dir.path(), "test").unwrap();
        assert_eq!(index.symbols_named("alpha").len(), 1);
        assert!(index.symbols_named("beta").is_empty());

        write(dir.path(), "src/a.ts", "export function beta() {}\n");
        let stats = apply_changes(
            &mut index,
            dir.path(),
            &[FileChange {
                path: PathBuf::from("src/a.ts"),
                kind: FileChangeKind::Modified,
            }],
        )
        .unwrap();
        assert_eq!(stats.files_updated, 1);
        assert_eq!(index.revision.dirty_generation, 1);
        assert!(index.symbols_named("alpha").is_empty());
        assert_eq!(index.symbols_named("beta").len(), 1);
    }

    #[test]
    fn delete_removes_file_record() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "src/gone.ts", "export function gone() {}\n");
        let (mut index, _) = build_index(dir.path(), "test").unwrap();
        assert!(!index.symbols_named("gone").is_empty());

        fs::remove_file(dir.path().join("src/gone.ts")).unwrap();
        apply_changes(
            &mut index,
            dir.path(),
            &[FileChange {
                path: PathBuf::from("src/gone.ts"),
                kind: FileChangeKind::Deleted,
            }],
        )
        .unwrap();
        assert!(index.symbols_named("gone").is_empty());
        assert!(!index.files.contains_key(Path::new("src/gone.ts")));
    }
}
