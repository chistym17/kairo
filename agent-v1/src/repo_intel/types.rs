//! Shared data types for hybrid repository localization.
//!
//! These shapes are the stable-ish contracts between indexers, sources,
//! merger, ranker, and snippet providers. Implementations evolve; prefer
//! additive changes over breaking renames.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Content identity foreshadowing persistent caches (v1 may leave unset).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FileHash(pub String);

/// Repository identity beyond a process-local counter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryRevision {
    pub workspace_id: String,
    pub git_head: Option<String>,
    pub dirty_generation: u64,
}

impl RepositoryRevision {
    pub fn new(workspace_id: impl Into<String>) -> Self {
        Self {
            workspace_id: workspace_id.into(),
            git_head: None,
            dirty_generation: 0,
        }
    }

    pub fn bump_dirty(&mut self) {
        self.dirty_generation = self.dirty_generation.saturating_add(1);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Language {
    Rust,
    Python,
    JavaScript,
    TypeScript,
    Other,
}

impl Language {
    pub fn from_extension(ext: Option<&str>) -> Self {
        match ext.map(|e| e.to_ascii_lowercase()).as_deref() {
            Some("rs") => Self::Rust,
            Some("py") => Self::Python,
            Some("js" | "jsx" | "mjs" | "cjs") => Self::JavaScript,
            Some("ts" | "tsx") => Self::TypeScript,
            _ => Self::Other,
        }
    }

    pub fn from_path(path: &Path) -> Self {
        Self::from_extension(path.extension().and_then(|e| e.to_str()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SymbolKind {
    Function,
    Class,
    Struct,
    Enum,
    Trait,
    Type,
    Const,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolRecord {
    pub name: String,
    pub kind: SymbolKind,
    /// 1-based line; 0 if unknown.
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolLocation {
    pub path: PathBuf,
    pub kind: SymbolKind,
    pub line: usize,
}

/// Best-effort textual import; unresolved targets stay as raw strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportReference {
    pub raw: String,
    /// Resolved local path when obvious; otherwise `None` (truthful).
    pub resolved: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct FieldTermFreqs {
    pub symbols: std::collections::HashMap<String, u32>,
    pub path: std::collections::HashMap<String, u32>,
    pub idents: std::collections::HashMap<String, u32>,
    pub strings: std::collections::HashMap<String, u32>,
}

impl FieldTermFreqs {
    pub fn total_len(&self) -> u32 {
        self.symbols.values().sum::<u32>()
            + self.path.values().sum::<u32>()
            + self.idents.values().sum::<u32>()
            + self.strings.values().sum::<u32>()
    }

    pub fn contains_any(&self, terms: &[String]) -> bool {
        terms.iter().any(|t| {
            let t = t.to_ascii_lowercase();
            self.symbols.contains_key(&t)
                || self.path.contains_key(&t)
                || self.idents.contains_key(&t)
                || self.strings.contains_key(&t)
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FileRecord {
    pub path: PathBuf,
    pub language: Option<Language>,
    pub content_hash: Option<FileHash>,
    pub tokens: Vec<String>,
    pub symbols: Vec<SymbolRecord>,
    pub imports: Vec<ImportReference>,
    /// Field term frequencies for BM25 (symbols / path / idents / strings).
    pub fields: FieldTermFreqs,
}

/// Observable evidence used by the ranker (extensible by adding variants).
#[derive(Debug, Clone, PartialEq)]
pub enum SignalKind {
    ExactSymbol,
    ExactToken,
    FilenameMatch,
    PathMatch,
    ImportNeighbor,
    DefinitionLike,
    LexicalHit,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CandidateSignal {
    pub kind: SignalKind,
    pub weight: f32,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snippet {
    pub line: usize,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RawCandidate {
    pub path: PathBuf,
    pub signals: Vec<CandidateSignal>,
    pub snippets: Vec<Snippet>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MergedCandidate {
    pub path: PathBuf,
    pub signals: Vec<CandidateSignal>,
    pub snippets: Vec<Snippet>,
}

/// Ranked locate hit for the hybrid pipeline (tool rendering may adapt later).
#[derive(Debug, Clone, PartialEq)]
pub struct LocateCandidate {
    pub path: PathBuf,
    pub score: f32,
    pub reasons: Vec<String>,
    pub snippets: Vec<Snippet>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileChangeKind {
    Modified,
    Created,
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    pub path: PathBuf,
    pub kind: FileChangeKind,
}

/// In-memory index. Build/scan logic comes in a later part; this is the schema.
#[derive(Debug, Clone, Default)]
pub struct RepositoryIndex {
    pub revision: RepositoryRevision,
    pub files: HashMap<PathBuf, FileRecord>,
    pub symbols_by_name: HashMap<String, Vec<SymbolLocation>>,
    pub files_by_token: HashMap<String, Vec<PathBuf>>,
    pub imports_by_file: HashMap<PathBuf, Vec<ImportReference>>,
}

impl Default for RepositoryRevision {
    fn default() -> Self {
        Self::new("default")
    }
}

impl RepositoryIndex {
    pub fn new(workspace_id: impl Into<String>) -> Self {
        Self {
            revision: RepositoryRevision::new(workspace_id),
            files: HashMap::new(),
            symbols_by_name: HashMap::new(),
            files_by_token: HashMap::new(),
            imports_by_file: HashMap::new(),
        }
    }

    pub fn file_count(&self) -> usize {
        self.files.len()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocateError {
    EmptyQuery,
    InvalidScope(String),
    Internal(String),
}

impl std::fmt::Display for LocateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyQuery => write!(f, "locate query must not be empty"),
            Self::InvalidScope(s) => write!(f, "invalid locate scope: {s}"),
            Self::Internal(s) => write!(f, "locate error: {s}"),
        }
    }
}

impl std::error::Error for LocateError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CandidateSourceError {
    Internal(String),
}

impl std::fmt::Display for CandidateSourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Internal(s) => write!(f, "candidate source error: {s}"),
        }
    }
}

impl std::error::Error for CandidateSourceError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepositoryIntelligenceError {
    UpdateFailed(String),
    Internal(String),
}

impl std::fmt::Display for RepositoryIntelligenceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UpdateFailed(s) => write!(f, "repository index update failed: {s}"),
            Self::Internal(s) => write!(f, "repository intelligence error: {s}"),
        }
    }
}

impl std::error::Error for RepositoryIntelligenceError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_from_extension() {
        assert_eq!(Language::from_extension(Some("ts")), Language::TypeScript);
        assert_eq!(Language::from_extension(Some("py")), Language::Python);
        assert_eq!(Language::from_extension(Some("rs")), Language::Rust);
        assert_eq!(Language::from_extension(Some("go")), Language::Other);
    }

    #[test]
    fn revision_bump_is_monotonic() {
        let mut rev = RepositoryRevision::new("ws");
        rev.bump_dirty();
        rev.bump_dirty();
        assert_eq!(rev.dirty_generation, 2);
    }

    #[test]
    fn empty_index_has_zero_files() {
        let idx = RepositoryIndex::new("ws");
        assert_eq!(idx.file_count(), 0);
        assert_eq!(idx.revision.dirty_generation, 0);
    }
}
