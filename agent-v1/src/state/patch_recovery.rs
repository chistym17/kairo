//! Recovery path after failed `apply_patch`.
//!
//! A hunk-mismatch (or other patch failure) is not the same as an unproductive
//! duplicate reread. We grant one targeted `read_file` of the failed path so the
//! model can refresh context and submit a smaller patch — without resetting
//! general exploration / stagnation counters.

use std::path::{Path, PathBuf};

use crate::state::stop_reason::StopReason;
use crate::tools::ToolResult;

/// Stop after this many consecutive `apply_patch` failures on the same file.
pub const MAX_CONSECUTIVE_PATCH_FAILURES: u32 = 3;

/// Stop after this many denied `write_file` attempts on existing source files.
pub const MAX_DENIED_SOURCE_WRITES: u32 = 2;

/// Extensions treated as source (existing files must use `apply_patch`).
const SOURCE_EXTENSIONS: &[&str] = &[
    "js", "mjs", "cjs", "ts", "tsx", "jsx", "py", "rs", "go", "java", "kt", "kts", "c",
    "cc", "cpp", "cxx", "h", "hpp", "hh", "rb", "php", "swift", "cs", "scala", "vue",
    "svelte",
];

#[derive(Debug, Default, Clone)]
pub struct PatchRecoveryState {
    /// One-shot reread grant for the last failed patch path.
    grant_path: Option<PathBuf>,
    grant_reads_left: u8,
    consecutive_failures: u32,
    consecutive_fail_path: Option<PathBuf>,
    denied_source_writes: u32,
}

impl PatchRecoveryState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn has_grant_for(&self, path: &Path) -> bool {
        self.grant_reads_left > 0
            && self
                .grant_path
                .as_ref()
                .is_some_and(|p| paths_equal(p, path))
    }

    /// Consume one recovery read if this path matches the grant.
    /// Returns true when the read should bypass duplicate-read denial.
    pub fn try_consume_read(&mut self, path: &Path) -> bool {
        if !self.has_grant_for(path) {
            return false;
        }
        self.grant_reads_left = self.grant_reads_left.saturating_sub(1);
        if self.grant_reads_left == 0 {
            self.grant_path = None;
        }
        true
    }

    /// Record a successful mutation — clears grants and consecutive fail streak.
    pub fn note_successful_mutation(&mut self) {
        self.grant_path = None;
        self.grant_reads_left = 0;
        self.consecutive_failures = 0;
        self.consecutive_fail_path = None;
    }

    /// After a failed `apply_patch`: grant one reread and maybe terminate.
    pub fn after_patch_failure(&mut self, path: &Path, mut result: ToolResult) -> ToolResult {
        let path_buf = path.to_path_buf();
        if self
            .consecutive_fail_path
            .as_ref()
            .is_some_and(|p| paths_equal(p, path))
        {
            self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        } else {
            self.consecutive_fail_path = Some(path_buf.clone());
            self.consecutive_failures = 1;
        }

        self.grant_path = Some(path_buf.clone());
        self.grant_reads_left = 1;

        let recovery = format!(
            "\n\nPatch failed because the expected context did not match (or made no change).\n\
             You may reread this file once to refresh the relevant section:\n\
             {}\n\
             After rereading, submit a smaller apply_patch against the current content.\n\
             Do not rewrite the entire file with write_file.",
            path.display()
        );
        match &mut result.error {
            Some(e) => e.push_str(&recovery),
            None => result.error = Some(recovery.trim_start().to_string()),
        }

        if self.consecutive_failures >= MAX_CONSECUTIVE_PATCH_FAILURES {
            let msg = format!(
                "Stopped: {MAX_CONSECUTIVE_PATCH_FAILURES} consecutive apply_patch failures on {} \
                 without a successful edit. Regenerate from a fresh read or finish with blockers.",
                path.display()
            );
            if let Some(e) = &mut result.error {
                e.push_str("\n\n");
                e.push_str(&msg);
            }
            result.stop_reason = Some(StopReason::NoProgress);
        }

        result
    }

    /// Deny `write_file` when the path already exists as source code.
    pub fn deny_write_existing_source(
        &mut self,
        workspace_root: &Path,
        path: &Path,
    ) -> Option<ToolResult> {
        if !is_source_path(path) {
            return None;
        }
        let full = workspace_root.join(path);
        if !full.is_file() {
            return None;
        }

        self.denied_source_writes = self.denied_source_writes.saturating_add(1);

        let mut message = format!(
            "write_file denied for existing source file {}.\n\
             Use apply_patch for surgical edits. Do not replace the whole file.\n\
             If a patch just failed, read_file that path once (recovery grant), then apply_patch again.",
            path.display()
        );
        if self.has_grant_for(path) {
            message.push_str(&format!(
                "\nA recovery reread is currently available for {}.",
                path.display()
            ));
        }

        let mut result = ToolResult::error(message);
        if self.denied_source_writes >= MAX_DENIED_SOURCE_WRITES {
            result.error = Some(format!(
                "{}\n\nStopped: {MAX_DENIED_SOURCE_WRITES} denied write_file attempts on existing \
                 source files. Use apply_patch or finish with blockers.",
                result.error.as_deref().unwrap_or("")
            ));
            result.stop_reason = Some(StopReason::NoProgress);
        }
        Some(result)
    }
}

pub fn is_source_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| SOURCE_EXTENSIONS.iter().any(|s| e.eq_ignore_ascii_case(s)))
        .unwrap_or(false)
}

fn paths_equal(a: &Path, b: &Path) -> bool {
    normalize(a) == normalize(b)
}

fn normalize(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .trim_start_matches("./")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn grant_allows_one_read_then_expires() {
        let mut s = PatchRecoveryState::new();
        let path = Path::new("src/store.js");
        let _ = s.after_patch_failure(path, ToolResult::error("hunk mismatch"));
        assert!(s.has_grant_for(path));
        assert!(s.try_consume_read(path));
        assert!(!s.has_grant_for(path));
        assert!(!s.try_consume_read(path));
    }

    #[test]
    fn third_consecutive_failure_stops() {
        let mut s = PatchRecoveryState::new();
        let path = Path::new("a.js");
        for i in 1..=3 {
            let r = s.after_patch_failure(path, ToolResult::error(format!("fail {i}")));
            if i < 3 {
                assert!(r.stop_reason.is_none());
            } else {
                assert_eq!(r.stop_reason, Some(StopReason::NoProgress));
            }
        }
    }

    #[test]
    fn success_clears_streak() {
        let mut s = PatchRecoveryState::new();
        let path = Path::new("a.js");
        let _ = s.after_patch_failure(path, ToolResult::error("1"));
        let _ = s.after_patch_failure(path, ToolResult::error("2"));
        s.note_successful_mutation();
        let r = s.after_patch_failure(path, ToolResult::error("again"));
        assert_eq!(s.consecutive_failures, 1);
        assert!(r.stop_reason.is_none());
    }

    #[test]
    fn denies_write_on_existing_js() {
        let dir = tempdir().unwrap();
        let rel = Path::new("src/store.js");
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join(rel), "export const x = 1;\n").unwrap();
        let mut s = PatchRecoveryState::new();
        let denied = s.deny_write_existing_source(dir.path(), rel);
        assert!(denied.is_some());
        assert!(!denied.unwrap().success);
    }

    #[test]
    fn allows_write_for_new_source_file() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        let mut s = PatchRecoveryState::new();
        assert!(s
            .deny_write_existing_source(dir.path(), Path::new("src/new.js"))
            .is_none());
    }

    #[test]
    fn allows_write_for_existing_json() {
        let dir = tempdir().unwrap();
        let rel = Path::new("package.json");
        fs::write(dir.path().join(rel), "{}\n").unwrap();
        let mut s = PatchRecoveryState::new();
        assert!(s.deny_write_existing_source(dir.path(), rel).is_none());
    }
}
