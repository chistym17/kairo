use std::collections::HashMap;
use std::path::Path;

use crate::tools::ToolResult;

/// Max times the same verify command may be returned without an intervening edit:
/// 1 = real execution, 2 = cached replay, 3+ = blocked.
pub const MAX_IDENTICAL_SHELL_WITHOUT_EDIT: u32 = 2;

/// Per-run dedup for `run_shell` / `run_command` (Phase 10.5).
/// Stops identical verify-thrash like the triangle-bug run (same test 30×).
#[derive(Debug, Default)]
pub struct ShellCommandCache {
    entries: HashMap<ShellKey, ShellEntry>,
    pub cache_hits: u32,
    pub blocked: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ShellKey {
    kind: ShellKind,
    command: String,
    cwd: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ShellKind {
    Shell,
    Command,
}

#[derive(Debug, Clone)]
struct ShellEntry {
    result: ToolResult,
    /// How many times this exact command was observed since last edit
    /// (including the first live execution).
    times_seen: u32,
    first_step: u32,
}

impl ShellCommandCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Before executing: if this command was already run since the last edit,
    /// return a cache hit or a hard block (no more identical runs).
    pub fn before_execute(
        &mut self,
        kind: &str,
        command: &str,
        cwd: &Path,
    ) -> Option<ToolResult> {
        let key = ShellKey {
            kind: parse_kind(kind)?,
            command: normalize_command(command),
            cwd: normalize_cwd(cwd),
        };
        let entry = self.entries.get_mut(&key)?;
        entry.times_seen += 1;

        if entry.times_seen <= MAX_IDENTICAL_SHELL_WITHOUT_EDIT {
            self.cache_hits += 1;
            let mut out = entry.result.clone();
            out.output = format!(
                "[shell-cache — identical command already ran at step {} — not re-executed]\n\
                 Do not run this same command again. If verification already succeeded, call finish. \
                 If it failed, change the approach (edit code or run a different check).\n\n{}",
                entry.first_step, out.output
            );
            Some(out)
        } else {
            self.blocked += 1;
            Some(ToolResult {
                success: false,
                output: String::new(),
                diff: None,
                error: Some(format!(
                    "Blocked: identical {} already returned {} times since the last edit \
                     (first at step {}). Do not retry it. Call finish if the task is done, \
                     or apply a new edit / different command.",
                    kind_label(key.kind),
                    entry.times_seen,
                    entry.first_step
                )),
                stop_reason: None,
            })
        }
    }

    /// After a live execution, store the result as the canonical outcome for this key.
    pub fn store_live(
        &mut self,
        kind: &str,
        command: &str,
        cwd: &Path,
        first_step: u32,
        result: &ToolResult,
    ) {
        let Some(kind) = parse_kind(kind) else {
            return;
        };
        let key = ShellKey {
            kind,
            command: normalize_command(command),
            cwd: normalize_cwd(cwd),
        };
        self.entries.insert(
            key,
            ShellEntry {
                result: result.clone(),
                times_seen: 1,
                first_step,
            },
        );
    }

    /// Clears after any successful file mutation so verifies can run fresh.
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

fn parse_kind(kind: &str) -> Option<ShellKind> {
    match kind {
        "run_shell" => Some(ShellKind::Shell),
        "run_command" => Some(ShellKind::Command),
        _ => None,
    }
}

fn kind_label(kind: ShellKind) -> &'static str {
    match kind {
        ShellKind::Shell => "run_shell",
        ShellKind::Command => "run_command",
    }
}

fn normalize_command(command: &str) -> String {
    command.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn normalize_cwd(cwd: &Path) -> String {
    let s = cwd.to_string_lossy();
    if s.is_empty() || s == "." {
        ".".to_string()
    } else {
        s.replace('\\', "/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn ok(out: &str) -> ToolResult {
        ToolResult {
            success: true,
            output: out.to_string(),
            diff: None,
            error: None,
            stop_reason: None,
        }
    }

    #[test]
    fn first_replay_is_cache_second_is_blocked() {
        let mut cache = ShellCommandCache::new();
        let cwd = PathBuf::from(".");
        let cmd = "cd backend && ./manage.py test";

        cache.store_live("run_shell", cmd, &cwd, 5, &ok("exit 0\nOK"));

        let hit = cache.before_execute("run_shell", cmd, &cwd).unwrap();
        assert!(hit.success);
        assert!(hit.output.contains("shell-cache"));
        assert!(hit.output.contains("OK"));
        assert_eq!(cache.cache_hits, 1);

        let blocked = cache.before_execute("run_shell", cmd, &cwd).unwrap();
        assert!(!blocked.success);
        assert!(blocked.error.as_ref().unwrap().contains("Blocked"));
        assert_eq!(cache.blocked, 1);
    }

    #[test]
    fn whitespace_normalized_as_same_command() {
        let mut cache = ShellCommandCache::new();
        let cwd = PathBuf::from(".");
        cache.store_live("run_shell", "ls   -la", &cwd, 0, &ok("a"));
        assert!(cache
            .before_execute("run_shell", "ls -la", &cwd)
            .unwrap()
            .success);
    }

    #[test]
    fn clear_allows_rerun_after_edit() {
        let mut cache = ShellCommandCache::new();
        let cwd = PathBuf::from(".");
        let cmd = "pytest";
        cache.store_live("run_shell", cmd, &cwd, 1, &ok("pass"));
        let _ = cache.before_execute("run_shell", cmd, &cwd);
        let _ = cache.before_execute("run_shell", cmd, &cwd); // blocked

        cache.clear();
        assert!(cache.before_execute("run_shell", cmd, &cwd).is_none());
    }

    #[test]
    fn different_commands_are_independent() {
        let mut cache = ShellCommandCache::new();
        let cwd = PathBuf::from(".");
        cache.store_live("run_shell", "ls", &cwd, 0, &ok("ls"));
        assert!(cache.before_execute("run_shell", "pwd", &cwd).is_none());
    }
}
