use std::collections::HashMap;
use std::path::{Component, Path};
use std::time::UNIX_EPOCH;

use crate::tools::ToolResult;

/// Third identical unchanged read of the same range is blocked.
pub const MAX_IDENTICAL_READ_WITHOUT_EDIT: u32 = 2;

/// Overlap ratio (0–1) above which a new read counts as mostly repeated.
pub const READ_OVERLAP_THRESHOLD: f64 = 0.8;

/// Per-run cache of successful `read_file` windows.
#[derive(Debug, Default)]
pub struct ReadCache {
    entries: HashMap<CacheKey, CachedRead>,
    /// Per path+stamp: line ranges already covered (for overlap detection).
    path_windows: HashMap<PathStampKey, Vec<LineWindow>>,
    pub hits: u32,
    pub misses: u32,
    pub repeated_reads: u32,
    pub blocked_repeated_reads: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CacheKey {
    path: String,
    offset: u32,
    limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PathStampKey {
    path: String,
    stamp: FileStamp,
}

#[derive(Debug, Clone, Copy)]
struct LineWindow {
    offset: u32,
    limit: u32,
    times_requested: u32,
}

#[derive(Debug, Clone)]
struct CachedRead {
    stamp: FileStamp,
    output: String,
    first_step: u32,
    /// Requests for this exact unchanged range (1 = first live read stored).
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
    BlockedRepeated,
}

impl ReadCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Check cache / repeat policy before a live read.
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
            path: path_key.clone(),
            offset,
            limit,
        };

        if let Some(entry) = self.entries.get_mut(&key) {
            if entry.stamp == stamp {
                entry.times_requested = entry.times_requested.saturating_add(1);
                if entry.times_requested > MAX_IDENTICAL_READ_WITHOUT_EDIT {
                    self.blocked_repeated_reads += 1;
                    return ReadCacheOutcome::BlockedRepeated;
                }
                self.hits += 1;
                self.repeated_reads += 1;
                return ReadCacheOutcome::Hit;
            }
            self.entries.remove(&key);
        }

        // Overlap with a prior window on the same unchanged file.
        let path_stamp_key = PathStampKey {
            path: path_key.clone(),
            stamp,
        };
        if let Some(windows) = self.path_windows.get_mut(&path_stamp_key) {
            for window in windows.iter_mut() {
                let ratio = range_overlap_ratio(offset, limit, window.offset, window.limit);
                if ratio >= READ_OVERLAP_THRESHOLD {
                    window.times_requested = window.times_requested.saturating_add(1);
                    if window.times_requested > MAX_IDENTICAL_READ_WITHOUT_EDIT {
                        self.blocked_repeated_reads += 1;
                        return ReadCacheOutcome::BlockedRepeated;
                    }
                    self.hits += 1;
                    self.repeated_reads += 1;
                    return ReadCacheOutcome::Hit;
                }
            }
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
            ReadCacheOutcome::BlockedRepeated => Some(repeated_read_block_message(path, offset, limit)),
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
            path: path_key.clone(),
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

        let path_stamp_key = PathStampKey {
            path: path_key,
            stamp,
        };
        let windows = self.path_windows.entry(path_stamp_key).or_default();
        if let Some(existing) = windows
            .iter_mut()
            .find(|w| w.offset == offset && w.limit == limit)
        {
            existing.times_requested = existing.times_requested.max(1);
        } else {
            windows.push(LineWindow {
                offset,
                limit,
                times_requested: 1,
            });
        }
    }

    pub fn invalidate_path(&mut self, path: &Path) {
        let key_path = normalize_rel(path);
        self.entries.retain(|k, _| k.path != key_path);
        self.path_windows
            .retain(|k, _| k.path != key_path);
    }

    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }
}

pub fn repeated_read_block_message(path: &Path, offset: u32, limit: u32) -> ToolResult {
    ToolResult {
        success: false,
        output: String::new(),
        diff: None,
        error: Some(format!(
            "Governance: repeated read blocked for {} offset={} limit={}. \
             This exact unchanged range (or an 80%+ overlapping range) was already provided. \
             Use the existing context, read a different range, edit the file, or verify.",
            path.display(),
            offset,
            limit
        )),
        stop_reason: None,
    }
}

fn range_overlap_ratio(a_offset: u32, a_limit: u32, b_offset: u32, b_limit: u32) -> f64 {
    if a_limit == 0 || b_limit == 0 {
        return 0.0;
    }
    let a_start = a_offset as u64;
    let a_end = a_offset.saturating_add(a_limit) as u64;
    let b_start = b_offset as u64;
    let b_end = b_offset.saturating_add(b_limit) as u64;
    let overlap = a_end.min(b_end).saturating_sub(a_start.max(b_start));
    let min_len = (a_limit as u64).min(b_limit as u64);
    if min_len == 0 {
        0.0
    } else {
        overlap as f64 / min_len as f64
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
    fn hit_on_second_identical_reread() {
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
        assert_eq!(cache.blocked_repeated_reads, 0);

        let blocked = cache.try_hit(dir.path(), path, 1, 10).unwrap();
        assert!(!blocked.success);
        assert!(blocked.error.unwrap().contains("repeated read blocked"));
        assert_eq!(cache.blocked_repeated_reads, 1);
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

    #[test]
    fn overlapping_range_counts_as_repeated() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "a\nb\nc\nd\ne\n").unwrap();
        let mut cache = ReadCache::new();
        let path = Path::new("a.txt");
        let r = read_file::execute(path, 1, 5, dir.path());
        cache.store(dir.path(), path, 1, 5, 0, &r.output);

        assert!(matches!(
            cache.before_read(dir.path(), path, 2, 4),
            ReadCacheOutcome::Hit
        ));
        assert!(matches!(
            cache.before_read(dir.path(), path, 2, 4),
            ReadCacheOutcome::BlockedRepeated
        ));
    }
}
