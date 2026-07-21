//! Bounded repository overview from index + snapshot (Phase 12).
//!
//! Goal: answer “what is this repo?” without recursive full-tree reads.
//! Not a perfect architecture document.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::types::{Language, RepositoryIndex};
use crate::tools::project_snapshot::ProjectSnapshot;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SummaryScope {
    Repository,
    Path(PathBuf),
    /// Soft label match against path segments / unit paths (best-effort).
    Subsystem(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SummaryDepth {
    #[default]
    Overview,
    Structural,
    Detailed,
}

impl SummaryDepth {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "overview" | "" => Some(Self::Overview),
            "structural" | "structure" => Some(Self::Structural),
            "detailed" | "detail" => Some(Self::Detailed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SummaryBudget {
    pub max_files: usize,
    pub max_symbols: usize,
    pub max_tokens: usize,
}

impl Default for SummaryBudget {
    fn default() -> Self {
        Self {
            max_files: 40,
            max_symbols: 40,
            max_tokens: 1200,
        }
    }
}

impl SummaryBudget {
    pub fn for_depth(depth: SummaryDepth) -> Self {
        match depth {
            SummaryDepth::Overview => Self {
                max_files: 24,
                max_symbols: 20,
                max_tokens: 900,
            },
            SummaryDepth::Structural => Self {
                max_files: 40,
                max_symbols: 40,
                max_tokens: 1400,
            },
            SummaryDepth::Detailed => Self {
                max_files: 60,
                max_symbols: 60,
                max_tokens: 2000,
            },
        }
    }

    fn max_chars(&self) -> usize {
        self.max_tokens.saturating_mul(4)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SummaryRequest {
    pub scope: SummaryScope,
    pub depth: SummaryDepth,
    pub budget: SummaryBudget,
    pub focus: Option<String>,
}

impl SummaryRequest {
    pub fn overview() -> Self {
        Self {
            scope: SummaryScope::Repository,
            depth: SummaryDepth::Overview,
            budget: SummaryBudget::for_depth(SummaryDepth::Overview),
            focus: None,
        }
    }

    pub fn with_depth(depth: SummaryDepth) -> Self {
        Self {
            scope: SummaryScope::Repository,
            depth,
            budget: SummaryBudget::for_depth(depth),
            focus: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositorySummary {
    pub text: String,
    pub truncated: bool,
    pub files_mentioned: usize,
    pub symbols_mentioned: usize,
}

/// Build a bounded overview from the hybrid index and optional project snapshot.
pub fn summarize(
    workspace_root: &Path,
    index: &RepositoryIndex,
    snapshot: Option<&ProjectSnapshot>,
    request: &SummaryRequest,
) -> RepositorySummary {
    let mut out = String::new();
    let mut truncated = false;
    let mut files_mentioned = 0usize;
    let mut symbols_mentioned = 0usize;
    let max_chars = request.budget.max_chars();

    out.push_str(&format!(
        "Repository summary ({:?}, scope={:?})\n",
        request.depth, request.scope
    ));
    out.push_str(&format!(
        "Indexed files: {}  revision dirty={}\n",
        index.file_count(),
        index.revision.dirty_generation
    ));

    if let Some(focus) = request.focus.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        out.push_str(&format!("Focus: {focus}\n"));
    }

    // ── Project units / manifests (snapshot) ─────────────────────────
    if let Some(snap) = snapshot {
        out.push_str("\n## Project units\n");
        let units: Vec<_> = snap
            .units
            .iter()
            .filter(|u| in_scope(&u.path, &request.scope))
            .collect();
        if units.is_empty() {
            out.push_str("(no matching units)\n");
        } else {
            for unit in units.iter().take(12) {
                let kind = unit
                    .kind
                    .map(|k| k.as_str())
                    .unwrap_or("unknown");
                let fw = unit.framework.as_deref().unwrap_or("-");
                out.push_str(&format!(
                    "- {}  kind={kind} framework={fw} manifests={}\n",
                    display_path(&unit.path),
                    unit.manifests.len()
                ));
                for m in unit.manifests.iter().take(4) {
                    out.push_str(&format!("    manifest: {}\n", display_path(m)));
                    files_mentioned = files_mentioned.saturating_add(1);
                }
                if matches!(request.depth, SummaryDepth::Structural | SummaryDepth::Detailed) {
                    for ep in unit.entry_points.iter().take(3) {
                        out.push_str(&format!(
                            "    entry: {} (score={})\n",
                            display_path(&ep.path),
                            ep.score
                        ));
                        files_mentioned = files_mentioned.saturating_add(1);
                    }
                    for cmd in unit.commands.iter().take(3) {
                        out.push_str(&format!(
                            "    cmd[{}]: {} {}\n",
                            cmd.label,
                            cmd.program,
                            cmd.args.join(" ")
                        ));
                    }
                }
            }
            if units.len() > 12 {
                truncated = true;
                out.push_str(&format!("… {} more units omitted\n", units.len() - 12));
            }
        }
        if snap.truncated {
            truncated = true;
            out.push_str("(project snapshot was truncated)\n");
        }
    }

    // ── Language / top-level layout from index ───────────────────────
    let scoped_files: Vec<_> = index
        .files
        .values()
        .filter(|f| in_scope(&f.path, &request.scope))
        .filter(|f| focus_match(&f.path.to_string_lossy(), request.focus.as_deref()))
        .collect();

    out.push_str("\n## Layout (indexed)\n");
    let mut lang_counts: HashMap<Language, usize> = HashMap::new();
    let mut top_dirs: HashMap<String, usize> = HashMap::new();
    for f in &scoped_files {
        let lang = f.language.unwrap_or(Language::Other);
        *lang_counts.entry(lang).or_default() += 1;
        let top = top_component(&f.path);
        *top_dirs.entry(top).or_default() += 1;
    }
    let mut langs: Vec<_> = lang_counts.into_iter().collect();
    langs.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| format!("{:?}", a.0).cmp(&format!("{:?}", b.0))));
    if !langs.is_empty() {
        out.push_str("Languages: ");
        out.push_str(
            &langs
                .iter()
                .take(8)
                .map(|(l, n)| format!("{}={n}", lang_label(*l)))
                .collect::<Vec<_>>()
                .join(", "),
        );
        out.push('\n');
    }
    let mut dirs: Vec<_> = top_dirs.into_iter().collect();
    dirs.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    out.push_str("Top-level paths:\n");
    for (dir, n) in dirs.iter().take(request.budget.max_files.min(16)) {
        out.push_str(&format!("  - {dir}/  ({n} indexed files)\n"));
        files_mentioned = files_mentioned.saturating_add(1);
    }
    if dirs.len() > 16 {
        truncated = true;
    }

    // ── Sample files (prefer manifests / entry-ish names) ────────────
    out.push_str("\n## Notable indexed paths\n");
    let mut ranked: Vec<_> = scoped_files.iter().map(|f| (file_salience(f), f)).collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.path.cmp(&b.1.path)));
    let file_cap = request.budget.max_files.min(20);
    for (_, f) in ranked.iter().take(file_cap) {
        out.push_str(&format!(
            "  - {}  [{}] symbols={} imports={}\n",
            display_path(&f.path),
            lang_label(f.language.unwrap_or(Language::Other)),
            f.symbols.len(),
            f.imports.len()
        ));
        files_mentioned = files_mentioned.saturating_add(1);
    }
    if ranked.len() > file_cap {
        truncated = true;
        out.push_str(&format!("… {} more files omitted\n", ranked.len() - file_cap));
    }

    // ── Symbols ──────────────────────────────────────────────────────
    if matches!(request.depth, SummaryDepth::Structural | SummaryDepth::Detailed)
        || request.focus.is_some()
    {
        out.push_str("\n## Symbols (sample)\n");
        let mut syms: Vec<(String, String, usize)> = Vec::new(); // name, path, line
        for f in &scoped_files {
            for s in &f.symbols {
                if focus_match(&s.name, request.focus.as_deref())
                    || request.focus.is_none()
                {
                    syms.push((
                        s.name.clone(),
                        display_path(&f.path),
                        s.line,
                    ));
                }
            }
        }
        syms.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        syms.dedup_by(|a, b| a.0 == b.0 && a.1 == b.1);
        let sym_cap = request.budget.max_symbols;
        for (name, path, line) in syms.iter().take(sym_cap) {
            out.push_str(&format!("  - {name}  @ {path}:{line}\n"));
            symbols_mentioned = symbols_mentioned.saturating_add(1);
        }
        if syms.len() > sym_cap {
            truncated = true;
            out.push_str(&format!("… {} more symbols omitted\n", syms.len() - sym_cap));
        }
    }

    // ── Import-heavy files (structural+) ─────────────────────────────
    if matches!(request.depth, SummaryDepth::Structural | SummaryDepth::Detailed) {
        out.push_str("\n## Import-heavy files\n");
        let mut heavy: Vec<_> = scoped_files
            .iter()
            .map(|f| (f.imports.len(), f))
            .filter(|(n, _)| *n > 0)
            .collect();
        heavy.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.path.cmp(&b.1.path)));
        for (n, f) in heavy.iter().take(8) {
            out.push_str(&format!("  - {}  ({n} imports)\n", display_path(&f.path)));
            files_mentioned = files_mentioned.saturating_add(1);
        }
        if heavy.is_empty() {
            out.push_str("  (none recorded)\n");
        }
    }

    // ── README excerpt ───────────────────────────────────────────────
    out.push_str("\n## README excerpt\n");
    match read_readme_excerpt(workspace_root, request.budget.max_chars().min(800)) {
        Some((name, excerpt)) => {
            out.push_str(&format!("From {name}:\n{excerpt}\n"));
            files_mentioned = files_mentioned.saturating_add(1);
        }
        None => out.push_str("(no README found at workspace root)\n"),
    }

    out.push_str(
        "\n(Bounded summary — use locate/read_file for specifics; do not recursively read the tree.)\n",
    );

    if out.len() > max_chars {
        truncated = true;
        out.truncate(max_chars);
        out.push_str("\n…[summary truncated to budget]\n");
    }

    RepositorySummary {
        text: out,
        truncated,
        files_mentioned,
        symbols_mentioned,
    }
}

fn in_scope(path: &Path, scope: &SummaryScope) -> bool {
    match scope {
        SummaryScope::Repository => true,
        SummaryScope::Path(prefix) => {
            if prefix.as_os_str().is_empty() || prefix == Path::new(".") {
                true
            } else {
                path.starts_with(prefix)
            }
        }
        SummaryScope::Subsystem(label) => {
            let needle = label.to_ascii_lowercase();
            path.to_string_lossy().to_ascii_lowercase().contains(&needle)
        }
    }
}

fn focus_match(text: &str, focus: Option<&str>) -> bool {
    let Some(focus) = focus.map(str::trim).filter(|s| !s.is_empty()) else {
        return true;
    };
    let t = text.to_ascii_lowercase();
    focus
        .split_whitespace()
        .any(|w| !w.is_empty() && t.contains(&w.to_ascii_lowercase()))
}

fn top_component(path: &Path) -> String {
    path.components()
        .next()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .filter(|s| !s.is_empty() && s != ".")
        .unwrap_or_else(|| ".".to_string())
}

fn display_path(path: &Path) -> String {
    if path.as_os_str().is_empty() || path == Path::new(".") {
        ".".to_string()
    } else {
        path.display().to_string()
    }
}

fn lang_label(lang: Language) -> &'static str {
    match lang {
        Language::Rust => "rust",
        Language::Python => "python",
        Language::JavaScript => "js",
        Language::TypeScript => "ts",
        Language::Other => "other",
    }
}

fn file_salience(f: &super::types::FileRecord) -> i32 {
    let name = f
        .path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let mut score = 0i32;
    for hint in [
        "readme",
        "package.json",
        "cargo.toml",
        "pyproject.toml",
        "manage.py",
        "main.",
        "index.",
        "app.",
        "settings",
        "routes",
        "api.",
    ] {
        if name.contains(hint.trim_end_matches('.')) || name == hint {
            score += 50;
        }
    }
    score += (f.symbols.len() as i32).min(20);
    score += (f.imports.len() as i32).min(10);
    score
}

fn read_readme_excerpt(workspace_root: &Path, max_chars: usize) -> Option<(String, String)> {
    const CANDIDATES: &[&str] = &[
        "README.md",
        "Readme.md",
        "readme.md",
        "README.rst",
        "README.txt",
        "README",
    ];
    for name in CANDIDATES {
        let path = workspace_root.join(name);
        if !path.is_file() {
            continue;
        }
        let raw = fs::read_to_string(&path).ok()?;
        let excerpt = truncate_chars(raw.trim(), max_chars);
        return Some((name.to_string(), excerpt));
    }
    None
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo_intel::types::{FileRecord, FieldTermFreqs, RepositoryRevision, SymbolKind, SymbolRecord};
    use crate::tools::project_snapshot::{ProjectKind, ProjectSnapshot, ProjectUnit};

    fn sample_index() -> RepositoryIndex {
        let mut index = RepositoryIndex {
            revision: RepositoryRevision::new("test"),
            ..Default::default()
        };
        index.files.insert(
            PathBuf::from("frontend/src/api.ts"),
            FileRecord {
                path: PathBuf::from("frontend/src/api.ts"),
                language: Some(Language::TypeScript),
                content_hash: None,
                tokens: vec![],
                symbols: vec![SymbolRecord {
                    name: "getTasks".into(),
                    kind: SymbolKind::Function,
                    line: 10,
                }],
                imports: vec![],
                fields: FieldTermFreqs::default(),
            },
        );
        index.files.insert(
            PathBuf::from("backend/manage.py"),
            FileRecord {
                path: PathBuf::from("backend/manage.py"),
                language: Some(Language::Python),
                content_hash: None,
                tokens: vec![],
                symbols: vec![],
                imports: vec![],
                fields: FieldTermFreqs::default(),
            },
        );
        index
    }

    #[test]
    fn overview_is_bounded_and_mentions_layout() {
        let index = sample_index();
        let snap = ProjectSnapshot {
            root: PathBuf::from("."),
            units: vec![ProjectUnit {
                path: PathBuf::from("frontend"),
                kind: Some(ProjectKind::Frontend),
                framework: Some("next".into()),
                manifests: vec![PathBuf::from("frontend/package.json")],
                commands: vec![],
                entry_points: vec![],
                confidence: 0.9,
                evidence: vec![],
            }],
            truncated: false,
        };
        let summary = summarize(
            Path::new("/tmp"),
            &index,
            Some(&snap),
            &SummaryRequest::overview(),
        );
        assert!(summary.text.contains("frontend"));
        assert!(summary.text.contains("Languages:"));
        assert!(summary.text.contains("Bounded summary"));
        assert!(summary.text.len() < 20_000);
    }

    #[test]
    fn structural_includes_symbols() {
        let index = sample_index();
        let summary = summarize(
            Path::new("/tmp"),
            &index,
            None,
            &SummaryRequest::with_depth(SummaryDepth::Structural),
        );
        assert!(summary.text.contains("getTasks"));
        assert!(summary.symbols_mentioned >= 1);
    }
}
