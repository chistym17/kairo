//! First RepositoryIntelligence backend: content search + snapshot entry points.

use std::fs;
use std::path::{Path, PathBuf};

use crate::tools::ignore::is_ignored_dir_name;
use crate::tools::project_snapshot::ProjectSnapshot;

use super::{
    Candidate, LocateRequest, LocateResult, MatchReason, RepositoryIntelligence, MAX_CANDIDATES,
    MAX_RAW_HITS, MAX_SNIPPET_CHARS,
};

const MAX_FILE_BYTES: u64 = 512 * 1024;

#[derive(Debug, Clone)]
struct RawHit {
    path: PathBuf,
    line: usize,
    text: String,
}

pub struct SearchSnapshotIntel<'a> {
    workspace_root: &'a Path,
    snapshot: &'a ProjectSnapshot,
}

impl<'a> SearchSnapshotIntel<'a> {
    pub fn new(workspace_root: &'a Path, snapshot: &'a ProjectSnapshot) -> Self {
        Self {
            workspace_root,
            snapshot,
        }
    }
}

impl RepositoryIntelligence for SearchSnapshotIntel<'_> {
    fn locate(&self, request: &LocateRequest) -> LocateResult {
        let query = request.query.trim();
        let scope = if request.scope.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            request.scope.clone()
        };

        let mut truncated = false;
        let mut raw = Vec::new();
        let root = if scope == Path::new(".") {
            self.workspace_root.to_path_buf()
        } else {
            self.workspace_root.join(&scope)
        };

        let _ = walk_collect(
            self.workspace_root,
            &root,
            &query.to_ascii_lowercase(),
            &mut raw,
            &mut truncated,
        );

        // Path-only hits: filename contains query even if content did not.
        let path_hits = collect_path_hits(self.workspace_root, &root, query);
        for ph in path_hits {
            if !raw.iter().any(|h| h.path == ph.path) {
                raw.push(ph);
            }
        }

        let entry_paths: Vec<PathBuf> = self
            .snapshot
            .units
            .iter()
            .flat_map(|u| u.entry_points.iter().map(|e| e.path.clone()))
            .collect();

        let mut ranked: Vec<Candidate> = raw
            .into_iter()
            .map(|hit| score_hit(&hit, query, &entry_paths))
            .collect();

        ranked.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.path.cmp(&b.path))
                .then_with(|| a.line.cmp(&b.line))
        });

        // Dedup identical path:line; keep highest score (already sorted).
        let mut seen = std::collections::BTreeSet::new();
        ranked.retain(|c| seen.insert((c.path.clone(), c.line)));

        if ranked.len() > MAX_CANDIDATES {
            ranked.truncate(MAX_CANDIDATES);
            truncated = true;
        }

        LocateResult {
            query: query.to_string(),
            scope,
            candidates: ranked,
            truncated,
        }
    }
}

fn score_hit(hit: &RawHit, query: &str, entry_paths: &[PathBuf]) -> Candidate {
    let q_lower = query.to_ascii_lowercase();
    let line_lower = hit.text.to_ascii_lowercase();
    let path_str = hit.path.to_string_lossy().replace('\\', "/");
    let path_lower = path_str.to_ascii_lowercase();
    let file_name = hit
        .path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    let mut score: f32 = 1.0;
    let mut reason = MatchReason::NearbyMatch;

    let exact = has_exact_token(&line_lower, &q_lower)
        || (!hit.text.is_empty() && line_lower.trim() == q_lower);
    if exact {
        score += 10.0;
        reason = MatchReason::ExactMatch;
    }

    if looks_definition_like(&line_lower, &q_lower) {
        score += 8.0;
        if matches!(
            reason,
            MatchReason::NearbyMatch | MatchReason::PathMatch | MatchReason::EntryPoint
        ) {
            reason = MatchReason::DefinitionLike;
        } else if reason == MatchReason::ExactMatch {
            // Prefer definition_like as the displayed reason when both apply —
            // it is more actionable for the model.
            reason = MatchReason::DefinitionLike;
        }
    }

    let path_match = file_name.contains(&q_lower) || path_lower.contains(&q_lower);
    if path_match {
        score += 5.0;
        if reason == MatchReason::NearbyMatch {
            reason = MatchReason::PathMatch;
        }
    }

    let is_entry = entry_paths.iter().any(|ep| {
        let ep_s = ep.to_string_lossy().replace('\\', "/");
        path_str == ep_s || path_str.ends_with(&ep_s) || ep_s.ends_with(&path_str)
    });
    if is_entry {
        score += 4.0;
        if reason == MatchReason::NearbyMatch {
            reason = MatchReason::EntryPoint;
        }
    }

    // Soft demotion for likely test files when the query does not look like a test.
    if is_test_path(&path_lower) && !q_lower.contains("test") {
        score -= 2.0;
    }

    let snippet = if hit.text.is_empty() {
        None
    } else {
        Some(clip_snippet(&hit.text))
    };

    Candidate {
        path: hit.path.clone(),
        line: hit.line,
        score,
        reason,
        snippet,
    }
}

fn has_exact_token(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let bytes = haystack.as_bytes();
    let n = needle.as_bytes();
    let mut i = 0;
    while i + n.len() <= bytes.len() {
        if bytes[i..i + n.len()].eq_ignore_ascii_case(n) {
            let before_ok = i == 0 || !is_ident_byte(bytes[i - 1]);
            let after = i + n.len();
            let after_ok = after >= bytes.len() || !is_ident_byte(bytes[after]);
            if before_ok && after_ok {
                return true;
            }
        }
        i += 1;
    }
    false
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn looks_definition_like(line_lower: &str, q_lower: &str) -> bool {
    if !has_exact_token(line_lower, q_lower) {
        return false;
    }
    let trimmed = line_lower.trim_start();
    const PREFIXES: &[&str] = &[
        "fn ",
        "pub fn ",
        "async fn ",
        "pub async fn ",
        "def ",
        "async def ",
        "class ",
        "struct ",
        "enum ",
        "type ",
        "interface ",
        "function ",
        "async function ",
        "export function ",
        "export async function ",
        "export const ",
        "export let ",
        "export var ",
        "export class ",
        "export default function ",
        "const ",
        "let ",
        "var ",
        "pub struct ",
        "pub enum ",
        "pub type ",
    ];
    PREFIXES.iter().any(|p| {
        if let Some(rest) = trimmed.strip_prefix(p) {
            rest.starts_with(q_lower)
                && (rest.len() == q_lower.len()
                    || !is_ident_byte(rest.as_bytes()[q_lower.len()]))
        } else {
            false
        }
    })
}

fn is_test_path(path_lower: &str) -> bool {
    path_lower.contains("/test/")
        || path_lower.contains("/tests/")
        || path_lower.contains("__tests__")
        || path_lower.contains(".test.")
        || path_lower.contains(".spec.")
        || path_lower.ends_with("_test.py")
        || path_lower.ends_with("_test.rs")
        || path_lower.ends_with("_test.go")
}

fn clip_snippet(text: &str) -> String {
    let mut s = text.trim().to_string();
    if s.len() > MAX_SNIPPET_CHARS {
        s.truncate(MAX_SNIPPET_CHARS);
        s.push('…');
    }
    s
}

fn collect_path_hits(workspace_root: &Path, root: &Path, query: &str) -> Vec<RawHit> {
    let needle = query.to_ascii_lowercase();
    let mut out = Vec::new();
    let _ = walk_paths(workspace_root, root, &needle, &mut out);
    out
}

fn walk_paths(
    workspace_root: &Path,
    cur: &Path,
    needle: &str,
    out: &mut Vec<RawHit>,
) -> std::io::Result<()> {
    if out.len() >= 20 {
        return Ok(());
    }
    let md = match fs::metadata(cur) {
        Ok(m) => m,
        Err(_) => return Ok(()),
    };
    if md.is_file() {
        let name = cur
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if name.contains(needle) {
            if let Ok(rel) = cur.strip_prefix(workspace_root) {
                out.push(RawHit {
                    path: rel.to_path_buf(),
                    line: 0,
                    text: String::new(),
                });
            }
        }
        return Ok(());
    }
    for entry in fs::read_dir(cur)?.flatten() {
        let p = entry.path();
        if p.is_dir() {
            if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                if is_ignored_dir_name(name) {
                    continue;
                }
            }
            walk_paths(workspace_root, &p, needle, out)?;
        } else {
            walk_paths(workspace_root, &p, needle, out)?;
        }
    }
    Ok(())
}

fn walk_collect(
    workspace_root: &Path,
    cur: &Path,
    needle: &str,
    hits: &mut Vec<RawHit>,
    truncated: &mut bool,
) -> std::io::Result<()> {
    if hits.len() >= MAX_RAW_HITS {
        *truncated = true;
        return Ok(());
    }
    let md = match fs::metadata(cur) {
        Ok(m) => m,
        Err(_) => return Ok(()),
    };
    if md.is_file() {
        search_file(workspace_root, cur, needle, hits, truncated)?;
        return Ok(());
    }
    for entry in fs::read_dir(cur)?.flatten() {
        if hits.len() >= MAX_RAW_HITS {
            *truncated = true;
            break;
        }
        let p = entry.path();
        if p.is_dir() {
            if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                if is_ignored_dir_name(name) {
                    continue;
                }
            }
            walk_collect(workspace_root, &p, needle, hits, truncated)?;
        } else {
            search_file(workspace_root, &p, needle, hits, truncated)?;
        }
    }
    Ok(())
}

fn search_file(
    workspace_root: &Path,
    path: &Path,
    needle: &str,
    hits: &mut Vec<RawHit>,
    truncated: &mut bool,
) -> std::io::Result<()> {
    if hits.len() >= MAX_RAW_HITS {
        *truncated = true;
        return Ok(());
    }
    let meta = fs::metadata(path)?;
    if meta.len() > MAX_FILE_BYTES {
        return Ok(());
    }
    let bytes = fs::read(path)?;
    if bytes.contains(&0) {
        return Ok(());
    }
    let Ok(text) = String::from_utf8(bytes) else {
        return Ok(());
    };
    let rel = path
        .strip_prefix(workspace_root)
        .unwrap_or(path)
        .to_path_buf();

    for (idx, line) in text.lines().enumerate() {
        if hits.len() >= MAX_RAW_HITS {
            *truncated = true;
            break;
        }
        if line.to_ascii_lowercase().contains(needle) {
            hits.push(RawHit {
                path: rel.clone(),
                line: idx + 1,
                text: line.to_string(),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::project_snapshot::{
        EntryPoint, ProjectKind, ProjectSnapshot, ProjectUnit,
    };
    use std::fs;

    fn empty_snapshot() -> ProjectSnapshot {
        ProjectSnapshot {
            root: PathBuf::from("."),
            units: vec![],
            truncated: false,
        }
    }

    fn snapshot_with_entry(path: &str) -> ProjectSnapshot {
        ProjectSnapshot {
            root: PathBuf::from("."),
            truncated: false,
            units: vec![ProjectUnit {
                path: PathBuf::from("frontend"),
                kind: Some(ProjectKind::Frontend),
                framework: Some("Next.js".into()),
                manifests: vec![],
                commands: vec![],
                entry_points: vec![EntryPoint {
                    path: PathBuf::from(path),
                    score: 10,
                    evidence: vec!["convention".into()],
                }],
                confidence: 0.9,
                evidence: vec![],
            }],
        }
    }

    #[test]
    fn ranks_definition_above_nearby_and_test() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("frontend/src/lib")).unwrap();
        fs::create_dir_all(root.join("frontend/src/__tests__")).unwrap();
        fs::write(
            root.join("frontend/src/lib/api.ts"),
            "export async function moveTask(id: string, position: number) {\n  return id;\n}\n",
        )
        .unwrap();
        fs::write(
            root.join("frontend/src/__tests__/api.test.ts"),
            "it('calls moveTask', () => {\n  moveTask('1', 2);\n});\n",
        )
        .unwrap();
        fs::write(
            root.join("frontend/src/lib/other.ts"),
            "// see moveTask elsewhere\nconst x = 1;\n",
        )
        .unwrap();

        let snap = empty_snapshot();
        let intel = SearchSnapshotIntel::new(root, &snap);
        let result = intel.locate(&LocateRequest {
            query: "moveTask".into(),
            scope: PathBuf::from("."),
        });

        assert!(!result.candidates.is_empty());
        let top = &result.candidates[0];
        assert!(
            top.path.ends_with("api.ts"),
            "top was {:?}, candidates={:?}",
            top.path,
            result.candidates
        );
        assert_eq!(top.reason, MatchReason::DefinitionLike);
        assert!(top.snippet.as_ref().unwrap().contains("function moveTask"));

        // Test file should rank below definition when present.
        let def_idx = result
            .candidates
            .iter()
            .position(|c| c.path.ends_with("api.ts") && c.reason == MatchReason::DefinitionLike)
            .unwrap();
        if let Some(test_idx) = result
            .candidates
            .iter()
            .position(|c| c.path.to_string_lossy().contains("__tests__"))
        {
            assert!(def_idx < test_idx);
        }
    }

    #[test]
    fn entry_point_boost_applies() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("frontend/src/app")).unwrap();
        fs::write(
            root.join("frontend/src/app/page.tsx"),
            "export default function Page() { return null }\n",
        )
        .unwrap();
        fs::write(
            root.join("frontend/src/app/other.tsx"),
            "export default function Page() { return 1 }\n",
        )
        .unwrap();

        let snap = snapshot_with_entry("frontend/src/app/page.tsx");
        let intel = SearchSnapshotIntel::new(root, &snap);
        let result = intel.locate(&LocateRequest {
            query: "Page".into(),
            scope: PathBuf::from("."),
        });

        let page = result
            .candidates
            .iter()
            .find(|c| c.path.ends_with("page.tsx"))
            .expect("page.tsx candidate");
        assert!(page.score >= 10.0);
    }

    #[test]
    fn path_match_without_content() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/Navbar.tsx"), "export const x = 1;\n").unwrap();

        let snap = empty_snapshot();
        let intel = SearchSnapshotIntel::new(root, &snap);
        let result = intel.locate(&LocateRequest {
            query: "Navbar".into(),
            scope: PathBuf::from("."),
        });

        assert!(result
            .candidates
            .iter()
            .any(|c| c.path.ends_with("Navbar.tsx")));
    }
}
