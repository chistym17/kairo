use std::collections::HashMap;
use std::path::{Component, Path};
use std::time::UNIX_EPOCH;

use crate::tools::ToolResult;

/// Per-run cache of successful `read_file` windows.
/// Optimization only: repeated identical reads return cached content (never blocked).
#[derive(Debug, Default)]
pub struct ReadCache {
    entries: HashMap<CacheKey, CachedRead>,
    pub hits: u32,
    pub misses: u32,
    pub repeated_reads: u32,
    /// Retained for metrics compatibility; always 0 (blocking removed).
    pub blocked_repeated_reads: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CacheKey {
    path: String,
    offset: u32,
    limit: u32,
}

#[derive(Debug, Clone)]
struct CachedRead {
    stamp: FileStamp,
    output: String,
    first_step: u32,
    times_requested: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct FileStamp {
    len: u64,
    mtime_ns: u128,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadCacheOutcome {
    Miss,
    Hit,
}

impl ReadCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Check cache before a live read.
    pub fn before_read(
        &mut self,
        workspace_root: &Path,
        path: &Path,
        offset: u32,
        limit: u32,
    ) -> ReadCacheOutcome {
        let path_key = normalize_rel(path);
        let Some(stamp) = file_stamp(workspace_root, path) else {
            self.misses += 1;
            return ReadCacheOutcome::Miss;
        };

        let key = CacheKey {
            path: path_key,
            offset,
            limit,
        };

        if let Some(entry) = self.entries.get_mut(&key) {
            if entry.stamp == stamp {
                entry.times_requested = entry.times_requested.saturating_add(1);
                self.hits += 1;
                self.repeated_reads += 1;
                return ReadCacheOutcome::Hit;
            }
            self.entries.remove(&key);
        }

        self.misses += 1;
        ReadCacheOutcome::Miss
    }

    /// If the same unchanged range was already returned, yield a hit ToolResult.
    pub fn try_hit(
        &mut self,
        workspace_root: &Path,
        path: &Path,
        offset: u32,
        limit: u32,
    ) -> Option<ToolResult> {
        match self.before_read(workspace_root, path, offset, limit) {
            ReadCacheOutcome::Miss => None,
            ReadCacheOutcome::Hit => {
                let path_key = normalize_rel(path);
                let key = CacheKey {
                    path: path_key,
                    offset,
                    limit,
                };
                let entry = self.entries.get(&key)?;
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
        }
    }

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
        let path_key = normalize_rel(path);
        let key = CacheKey {
            path: path_key,
            offset,
            limit,
        };
        self.entries.insert(
            key,
            CachedRead {
                stamp,
                output: output.to_string(),
                first_step,
                times_requested: 1,
            },
        );
    }

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
    fn hit_on_repeated_identical_reread() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "one\ntwo\nthree\n").unwrap();

        let mut cache = ReadCache::new();
        let path = Path::new("a.txt");
        let fresh = read_file::execute(path, 1, 10, dir.path());
        assert!(fresh.success);
        cache.store(dir.path(), path, 1, 10, 0, &fresh.output);

        let hit = cache.try_hit(dir.path(), path, 1, 10).unwrap();
        assert!(hit.success);
        assert!(hit.output.contains("[cached"));
        assert_eq!(cache.hits, 1);

        let hit2 = cache.try_hit(dir.path(), path, 1, 10).unwrap();
        assert!(hit2.success);
        assert!(hit2.output.contains("[cached"));
        assert_eq!(cache.hits, 2);
        assert_eq!(cache.blocked_repeated_reads, 0);
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

        thread::sleep(Duration::from_millis(20));
        fs::write(&file, "v2\n").unwrap();

        assert!(matches!(
            cache.before_read(dir.path(), path, 1, 10),
            ReadCacheOutcome::Miss
        ));
    }
}
