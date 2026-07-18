//! Per-run memory of successful `read_file` windows.
//!
//! Unchanged duplicate / overlapping reads stay allowed but become cheap:
//! fully covered ranges return a short notice (no body dump); partial
//! overlaps return only unseen lines (small overlap for readability).

use std::collections::HashMap;
use std::path::{Component, Path};
use std::time::UNIX_EPOCH;

use crate::tools::ToolResult;

/// Soft overlap kept when returning only the unseen tail of a range.
const OVERLAP_LINES: u32 = 5;

#[derive(Debug, Default)]
pub struct ReadCache {
    /// Path → covered ranges while stamp matches.
    files: HashMap<String, FileReadMemory>,
    pub hits: u32,
    pub misses: u32,
    pub repeated_reads: u32,
    /// Retained for metrics compatibility; always 0 (blocking removed).
    pub blocked_repeated_reads: u32,
    /// Overlap-aware partial reads (only new lines returned).
    pub partial_reads: u32,
}

#[derive(Debug, Clone)]
struct FileReadMemory {
    stamp: FileStamp,
    /// From last successful `lines X-Y of N` header, when known.
    total_lines: Option<u32>,
    ranges: Vec<CoveredRange>,
}

#[derive(Debug, Clone)]
struct CoveredRange {
    start: u32,
    end: u32,
    first_step: u32,
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

/// How to satisfy a `read_file` without blocking the model.
#[derive(Debug)]
pub enum ReadResolve {
    /// Fully covered and unchanged — short notice, no file body.
    AlreadyRead(ToolResult),
    /// Live read needed (possibly narrowed to unseen lines).
    Live {
        offset: u32,
        limit: u32,
        /// Prepended to live output when this is a partial/overlap read.
        note: Option<String>,
    },
}

impl ReadCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Resolve a read: short notice, narrowed live window, or full live read.
    pub fn resolve(
        &mut self,
        workspace_root: &Path,
        path: &Path,
        offset: u32,
        limit: u32,
    ) -> ReadResolve {
        let req_start = offset.max(1);
        let req_end = req_start.saturating_add(limit.saturating_sub(1).max(0));
        let path_key = normalize_rel(path);
        let Some(stamp) = file_stamp(workspace_root, path) else {
            self.misses += 1;
            return ReadResolve::Live {
                offset: req_start,
                limit,
                note: None,
            };
        };

        // Drop stale memory if the file changed.
        if let Some(mem) = self.files.get(&path_key) {
            if mem.stamp != stamp {
                self.files.remove(&path_key);
            }
        }

        let Some(mem) = self.files.get(&path_key) else {
            self.misses += 1;
            return ReadResolve::Live {
                offset: req_start,
                limit,
                note: None,
            };
        };

        // Clamp to known file length so "read past EOF" still counts as covered.
        let req_end = mem
            .total_lines
            .map(|n| req_end.min(n.max(1)))
            .unwrap_or(req_end)
            .max(req_start);

        let covered = merge_ranges(&mem.ranges);
        if is_fully_covered(&covered, req_start, req_end) {
            let prior = mem
                .ranges
                .iter()
                .filter(|r| ranges_overlap(r.start, r.end, req_start, req_end))
                .map(|r| r.first_step)
                .min()
                .unwrap_or(0);
            self.hits = self.hits.saturating_add(1);
            self.repeated_reads = self.repeated_reads.saturating_add(1);
            return ReadResolve::AlreadyRead(already_read_result(
                &path_key, req_start, req_end, prior,
            ));
        }

        if let Some((new_start, new_end)) = first_unseen_span(&covered, req_start, req_end) {
            // Keep a little overlap so the model can reconnect context.
            let start = new_start.saturating_sub(OVERLAP_LINES).max(req_start);
            let end = new_end.min(req_end);
            if start > req_end || end < start {
                self.misses += 1;
                return ReadResolve::Live {
                    offset: req_start,
                    limit,
                    note: None,
                };
            }
            let lim = end.saturating_sub(start).saturating_add(1);
            let already_end = start.saturating_sub(1);
            let note = if already_end >= req_start {
                Some(format!(
                    "[overlap — {path_key} lines {req_start}-{already_end} already read and unchanged; returning {start}-{end}]\n"
                ))
            } else {
                Some(format!(
                    "[partial — {path_key} returning unseen lines {start}-{end} (file unchanged)]\n"
                ))
            };
            self.partial_reads = self.partial_reads.saturating_add(1);
            return ReadResolve::Live {
                offset: start,
                limit: lim,
                note,
            };
        }

        self.misses += 1;
        ReadResolve::Live {
            offset: req_start,
            limit,
            note: None,
        }
    }

    /// Backward-compatible exact-range hit helper used by older call sites/tests.
    pub fn try_hit(
        &mut self,
        workspace_root: &Path,
        path: &Path,
        offset: u32,
        limit: u32,
    ) -> Option<ToolResult> {
        match self.resolve(workspace_root, path, offset, limit) {
            ReadResolve::AlreadyRead(r) => Some(r),
            ReadResolve::Live { .. } => None,
        }
    }

    pub fn before_read(
        &mut self,
        workspace_root: &Path,
        path: &Path,
        offset: u32,
        limit: u32,
    ) -> ReadCacheOutcome {
        match self.resolve(workspace_root, path, offset, limit) {
            ReadResolve::AlreadyRead(_) => ReadCacheOutcome::Hit,
            ReadResolve::Live { .. } => ReadCacheOutcome::Miss,
        }
    }

    /// Record a successful read. Prefer actual returned `start..=end` when known.
    pub fn store_range(
        &mut self,
        workspace_root: &Path,
        path: &Path,
        start: u32,
        end: u32,
        first_step: u32,
    ) {
        self.store_range_with_total(workspace_root, path, start, end, first_step, None);
    }

    fn store_range_with_total(
        &mut self,
        workspace_root: &Path,
        path: &Path,
        start: u32,
        end: u32,
        first_step: u32,
        total_lines: Option<u32>,
    ) {
        if start == 0 || end < start {
            return;
        }
        let Some(stamp) = file_stamp(workspace_root, path) else {
            return;
        };
        let path_key = normalize_rel(path);
        let entry = self.files.entry(path_key).or_insert(FileReadMemory {
            stamp,
            total_lines: None,
            ranges: Vec::new(),
        });
        if entry.stamp != stamp {
            entry.stamp = stamp;
            entry.ranges.clear();
            entry.total_lines = None;
        }
        if total_lines.is_some() {
            entry.total_lines = total_lines;
        }
        entry.ranges.push(CoveredRange {
            start,
            end,
            first_step,
        });
        entry.ranges = merge_covered(entry.ranges.clone());
    }

    /// Store using requested offset/limit (fallback when returned range unknown).
    pub fn store(
        &mut self,
        workspace_root: &Path,
        path: &Path,
        offset: u32,
        limit: u32,
        first_step: u32,
        output: &str,
    ) {
        let (start, end, total) = parse_returned_meta(output).unwrap_or_else(|| {
            let start = offset.max(1);
            let end = start.saturating_add(limit.saturating_sub(1));
            (start, end, None)
        });
        self.store_range_with_total(workspace_root, path, start, end, first_step, total);
    }

    pub fn invalidate_path(&mut self, path: &Path) {
        let key_path = normalize_rel(path);
        self.files.remove(&key_path);
    }

    pub fn entry_count(&self) -> usize {
        self.files.values().map(|f| f.ranges.len()).sum()
    }
}

fn already_read_result(path: &str, start: u32, end: u32, prior_step: u32) -> ToolResult {
    ToolResult {
        success: true,
        output: format!(
            "[already read — {path} lines {start}-{end} were returned earlier (step {prior_step})]\n\
             File is unchanged since that read. Reuse the prior content; request a different range \
             or wait until the file is edited if you need a refresh."
        ),
        diff: None,
        error: None,
        stop_reason: None,
    }
}

fn parse_returned_meta(output: &str) -> Option<(u32, u32, Option<u32>)> {
    // "lines 185-224 of 247 — ..." (may follow an overlap note)
    for line in output.lines().take(8) {
        let rest = match line.strip_prefix("lines ") {
            Some(r) => r,
            None => continue,
        };
        let mut of_parts = rest.split(" of ");
        let range = of_parts.next()?;
        let total = of_parts
            .next()
            .and_then(|s| s.split_whitespace().next())
            .and_then(|s| s.parse().ok());
        let mut parts = range.split('-');
        let start: u32 = parts.next()?.parse().ok()?;
        let end: u32 = parts.next()?.parse().ok()?;
        return Some((start, end, total));
    }
    None
}

fn ranges_overlap(a0: u32, a1: u32, b0: u32, b1: u32) -> bool {
    a0 <= b1 && b0 <= a1
}

fn is_fully_covered(merged: &[(u32, u32)], start: u32, end: u32) -> bool {
    merged.iter().any(|(s, e)| *s <= start && *e >= end)
}

fn merge_ranges(ranges: &[CoveredRange]) -> Vec<(u32, u32)> {
    merge_covered(ranges.to_vec())
        .into_iter()
        .map(|r| (r.start, r.end))
        .collect()
}

fn merge_covered(mut ranges: Vec<CoveredRange>) -> Vec<CoveredRange> {
    if ranges.is_empty() {
        return ranges;
    }
    ranges.sort_by_key(|r| (r.start, r.end));
    let mut out = Vec::new();
    let mut cur = ranges[0].clone();
    for r in ranges.into_iter().skip(1) {
        if r.start <= cur.end.saturating_add(1) {
            cur.end = cur.end.max(r.end);
            cur.first_step = cur.first_step.min(r.first_step);
        } else {
            out.push(cur);
            cur = r;
        }
    }
    out.push(cur);
    out
}

/// First contiguous unseen span inside [req_start, req_end], if any.
fn first_unseen_span(
    covered: &[(u32, u32)],
    req_start: u32,
    req_end: u32,
) -> Option<(u32, u32)> {
    let mut cursor = req_start;
    for &(s, e) in covered {
        if e < cursor {
            continue;
        }
        if s > cursor {
            let end = (s.saturating_sub(1)).min(req_end);
            if cursor <= end {
                return Some((cursor, end));
            }
        }
        cursor = cursor.max(e.saturating_add(1));
        if cursor > req_end {
            return None;
        }
    }
    if cursor <= req_end {
        Some((cursor, req_end))
    } else {
        None
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
    fn identical_reread_returns_short_notice() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "one\ntwo\nthree\n").unwrap();

        let mut cache = ReadCache::new();
        let path = Path::new("a.txt");
        let fresh = read_file::execute(path, 1, 10, dir.path());
        assert!(fresh.success);
        cache.store(dir.path(), path, 1, 10, 0, &fresh.output);

        let hit = cache.try_hit(dir.path(), path, 1, 10).unwrap();
        assert!(hit.success);
        assert!(hit.output.contains("[already read"));
        assert!(!hit.output.contains("one\ntwo"));
        assert_eq!(cache.hits, 1);
        assert_eq!(cache.blocked_repeated_reads, 0);
    }

    #[test]
    fn subset_range_also_already_read() {
        let dir = tempfile::tempdir().unwrap();
        let body: String = (1..=50).map(|i| format!("L{i}\n")).collect();
        fs::write(dir.path().join("t.py"), &body).unwrap();

        let mut cache = ReadCache::new();
        let path = Path::new("t.py");
        let fresh = read_file::execute(path, 1, 250, dir.path());
        cache.store(dir.path(), path, 1, 250, 3, &fresh.output);

        match cache.resolve(dir.path(), path, 1, 100) {
            ReadResolve::AlreadyRead(r) => {
                assert!(r.output.contains("already read"));
                assert!(r.output.contains("step 3"));
            }
            other => panic!("expected AlreadyRead, got {other:?}"),
        }
    }

    #[test]
    fn overlap_returns_only_unseen_tail() {
        let dir = tempfile::tempdir().unwrap();
        let body: String = (1..=200).map(|i| format!("L{i}\n")).collect();
        fs::write(dir.path().join("t.py"), &body).unwrap();

        let mut cache = ReadCache::new();
        let path = Path::new("t.py");
        let fresh = read_file::execute(path, 1, 100, dir.path());
        cache.store(dir.path(), path, 1, 100, 1, &fresh.output);

        match cache.resolve(dir.path(), path, 70, 80) {
            ReadResolve::Live {
                offset,
                limit,
                note,
            } => {
                let note = note.expect("partial note");
                assert!(
                    note.contains("overlap") || note.contains("partial"),
                    "{note}"
                );
                // Unseen starts at 101; with 5-line overlap → offset ~96.
                assert!(offset >= 96, "offset={offset}");
                assert!(offset <= 101, "offset={offset}");
                assert!(limit >= 40, "limit={limit}");
            }
            other => panic!("expected Live partial, got {other:?}"),
        }
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
