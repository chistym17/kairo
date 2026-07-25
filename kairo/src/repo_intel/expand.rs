//! Best-effort import expansion around top candidates.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::types::{
    CandidateSignal, MergedCandidate, RepositoryIndex, SignalKind, Snippet,
};

const W_IMPORT_NEIGHBOR: f32 = 4.0;
const DEFAULT_TOP_N: usize = 3;

/// Expand obvious local imports / importers for the top-N paths.
pub fn expand_imports(
    index: &RepositoryIndex,
    ranked_paths: &[PathBuf],
    top_n: usize,
) -> Vec<MergedCandidate> {
    let n = if top_n == 0 { DEFAULT_TOP_N } else { top_n };
    let seeds: Vec<&PathBuf> = ranked_paths.iter().take(n).collect();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut out = Vec::new();

    for seed in seeds {
        seen.insert(seed.clone());
        // Outgoing imports from seed.
        for imp in index.imports_of(seed) {
            let Some(resolved) = &imp.resolved else {
                continue;
            };
            if !index.files.contains_key(resolved) {
                continue;
            }
            if !seen.insert(resolved.clone()) {
                continue;
            }
            out.push(neighbor(resolved, seed, &imp.raw));
        }
        // Incoming: files that import the seed.
        for (path, imports) in &index.imports_by_file {
            if seen.contains(path) {
                continue;
            }
            let hits = imports.iter().any(|i| {
                i.resolved
                    .as_ref()
                    .is_some_and(|r| paths_equal(r, seed))
            });
            if hits {
                seen.insert(path.clone());
                out.push(neighbor(path, seed, "importer"));
            }
        }
    }
    out
}

fn neighbor(path: &Path, seed: &Path, detail: &str) -> MergedCandidate {
    MergedCandidate {
        path: path.to_path_buf(),
        signals: vec![CandidateSignal {
            kind: SignalKind::ImportNeighbor,
            weight: W_IMPORT_NEIGHBOR,
            detail: Some(format!("{}←{}", detail, seed.display())),
        }],
        snippets: Vec::<Snippet>::new(),
    }
}

fn paths_equal(a: &Path, b: &Path) -> bool {
    a == b
        || a.to_string_lossy().replace('\\', "/") == b.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo_intel::indexer::build_index;
    use std::fs;

    #[test]
    fn expands_helper_from_tasks() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("api")).unwrap();
        fs::write(
            root.join("api/tasks.ts"),
            "import { helper } from './helper';\nexport function moveTask() {}\n",
        )
        .unwrap();
        fs::write(root.join("api/helper.ts"), "export const helper = 1;\n").unwrap();

        let (index, _) = build_index(root, "t").unwrap();
        let expanded = expand_imports(&index, &[PathBuf::from("api/tasks.ts")], 1);
        assert!(
            expanded.iter().any(|c| c.path.ends_with("helper.ts")),
            "{:?}",
            expanded
        );
    }
}
