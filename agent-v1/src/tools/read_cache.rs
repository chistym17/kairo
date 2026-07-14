use std::collections::HashMap;
use std::path::{Component, Path};
use std::time::UNIX_EPOCH;

use crate::tools::ToolResult;

/// Per-run cache of successful `read_file` windows.
#[derive(Debug, Default)]
pub struct ReadCache {
    entries: HashMap<CacheKey, CachedRead>,
    pub hits: u32,
    pub misses: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CacheKey {
    path: String,
    offset: u32,
    limit: u32,
}

#[derive(Debug, Clone)]
struct CachedRead {
    /// File size + mtime when the window was captured.
    stamp: FileStamp,
    output: String,
    first_step: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileStamp {
    len: u64,
    mtime_ns: u128,
}

impl ReadCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// If the same unchanged range was already returned, yield a hit ToolResult.
    pub fn try_hit(
        &mut self,
        workspace_root: &Path,
        path: &Path,
        offset: u32,
        limit: u32,
    ) -> Option<ToolResult> {
        let path_key = normalize_rel(path);
        let stamp = file_stamp(workspace_root, path)?;
        let key = CacheKey {
            path: path_key,
            offset,
            limit,
        };

        match self.entries.get(&key) {
            Some(entry) if entry.stamp == stamp => {
                self.hits += 1;
                let output = format!(
                    "[cached — same unchanged range as step {}]\n{}",
                    entry.first_step, entry.output
                );
                Some(ToolResult {
                    success: true,
                    output,
                    diff: None,
                    error: None,
                    stop_reason: None,
                })
            }
            Some(_) => {
                // Stale stamp — drop and miss.
                self.entries.remove(&key);
                self.misses += 1;
                None
            }
            None => {
                self.misses += 1;
                None
            }
        }
    }

    /// Store a successful read. No-op if stamp cannot be taken (file vanished, etc.).
    pub fn store(
        &mut self,
        workspace_root: &Path,
        path: &Path,
        offset: u32,
        limit: u32,
        first_step: u32,
        output: &str,
    ) {
        let Some(stamp) = file_stamp(workspace_root, path) else {
            return;
        };
        let key = CacheKey {
            path: normalize_rel(path),
            offset,
            limit,
        };
        self.entries.insert(
            key,
            CachedRead {
                stamp,
                output: output.to_string(),
                first_step,
            },
        );
    }

    /// Drop all cached windows for a relative path (after write/patch).
    pub fn invalidate_path(&mut self, path: &Path) {
        let key_path = normalize_rel(path);
        self.entries.retain(|k, _| k.path != key_path);
    }

    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }
}

fn normalize_rel(path: &Path) -> String {
    let mut parts = Vec::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                parts.pop();
            }
            Component::Normal(s) => parts.push(s.to_string_lossy().into_owned()),
            Component::RootDir | Component::Prefix(_) => {}
        }
    }
    if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    }
}

fn file_stamp(workspace_root: &Path, path: &Path) -> Option<FileStamp> {
    let full = workspace_root.join(path);
    let meta = std::fs::metadata(&full).ok()?;
    let mtime_ns = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    Some(FileStamp {
        len: meta.len(),
        mtime_ns,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::read_file;
    use std::fs;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn hit_on_identical_reread() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "one\ntwo\nthree\n").unwrap();

        let mut cache = ReadCache::new();
        let path = Path::new("a.txt");
        let fresh = read_file::execute(path, 1, 10, dir.path());
        assert!(fresh.success);
        cache.store(dir.path(), path, 1, 10, 0, &fresh.output);

        let hit = cache.try_hit(dir.path(), path, 1, 10).unwrap();
        assert!(hit.success);
        assert!(hit.output.contains("[cached — same unchanged range as step 0]"));
        assert!(hit.output.contains("one"));
        assert_eq!(cache.hits, 1);
        assert_eq!(cache.misses, 0);
    }

    #[test]
    fn miss_after_content_change() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        fs::write(&file, "v1\n").unwrap();

        let mut cache = ReadCache::new();
        let path = Path::new("a.txt");
        let fresh = read_file::execute(path, 1, 10, dir.path());
        cache.store(dir.path(), path, 1, 10, 1, &fresh.output);

        // Ensure mtime advances on filesystems with coarse resolution.
        thread::sleep(Duration::from_millis(20));
        fs::write(&file, "v2\n").unwrap();

        assert!(cache.try_hit(dir.path(), path, 1, 10).is_none());
        assert_eq!(cache.misses, 1);
        assert_eq!(cache.entry_count(), 0); // stale entry removed
    }

    #[test]
    fn invalidate_path_clears_windows() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "x\ny\nz\n").unwrap();

        let mut cache = ReadCache::new();
        let path = Path::new("a.txt");
        let r1 = read_file::execute(path, 1, 1, dir.path());
        let r2 = read_file::execute(path, 2, 1, dir.path());
        cache.store(dir.path(), path, 1, 1, 0, &r1.output);
        cache.store(dir.path(), path, 2, 1, 1, &r2.output);
        assert_eq!(cache.entry_count(), 2);

        cache.invalidate_path(path);
        assert_eq!(cache.entry_count(), 0);
        assert!(cache.try_hit(dir.path(), path, 1, 1).is_none());
    }

    #[test]
    fn different_ranges_are_separate_keys() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "a\nb\nc\n").unwrap();
        let mut cache = ReadCache::new();
        let path = Path::new("a.txt");
        let r = read_file::execute(path, 1, 1, dir.path());
        cache.store(dir.path(), path, 1, 1, 0, &r.output);

        assert!(cache.try_hit(dir.path(), path, 2, 1).is_none()); // miss
        assert!(cache.try_hit(dir.path(), path, 1, 1).is_some()); // hit
    }
}
