//! Line-based snippet provider.

use std::fs;
use std::path::{Path, PathBuf};

use super::traits::SnippetProvider;
use super::types::{LocateCandidate, Snippet};

const MAX_SNIPPET_CHARS: usize = 160;

pub struct LineSnippetProvider {
    pub workspace_root: PathBuf,
}

impl LineSnippetProvider {
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
        }
    }

    /// Ensure each candidate has up to `max_per` snippets from disk when missing.
    pub fn attach(&self, candidates: &mut [LocateCandidate], max_per: usize) {
        for c in candidates.iter_mut() {
            if !c.snippets.is_empty() {
                // Clip existing.
                for s in &mut c.snippets {
                    clip_in_place(&mut s.text);
                }
                if c.snippets.len() > max_per {
                    c.snippets.truncate(max_per);
                }
                continue;
            }
            let abs = self.workspace_root.join(&c.path);
            let Ok(content) = fs::read_to_string(abs) else {
                continue;
            };
            if let Some(snip) = self.snippet_for(&c.path, &content, None) {
                c.snippets.push(snip);
            }
        }
    }
}

impl SnippetProvider for LineSnippetProvider {
    fn snippet_for(&self, _path: &Path, content: &str, prefer_line: Option<usize>) -> Option<Snippet> {
        let lines: Vec<&str> = content.lines().collect();
        if lines.is_empty() {
            return None;
        }
        let idx = prefer_line
            .filter(|l| *l > 0 && *l <= lines.len())
            .map(|l| l - 1)
            .unwrap_or(0);
        let mut text = lines[idx].trim().to_string();
        clip_in_place(&mut text);
        Some(Snippet {
            line: idx + 1,
            text,
        })
    }
}

fn clip_in_place(s: &mut String) {
    if s.len() > MAX_SNIPPET_CHARS {
        s.truncate(MAX_SNIPPET_CHARS);
        s.push('…');
    }
}
