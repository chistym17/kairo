use std::collections::HashMap;
use std::path::Path;

use crate::tools::ToolResult;

/// Max times the same verify command may be returned without an intervening edit:
/// 1 = real execution, 2 = cached replay, 3+ = blocked.
pub const MAX_IDENTICAL_SHELL_WITHOUT_EDIT: u32 = 2;

/// After an edit, only this many explore shells (git/grep/sed/…) are allowed
/// until the next edit. Stops pathspec-thrash that bypasses exact-string dedup.
pub const MAX_EXPLORE_SHELLS_SINCE_EDIT: u32 = 5;

/// Per-run dedup + post-edit explore budget for `run_shell` / `run_command`.
#[derive(Debug, Default)]
pub struct ShellCommandCache {
    entries: HashMap<ShellKey, ShellEntry>,
    pub cache_hits: u32,
    pub blocked: u32,
    /// Explore-family shells executed (or attempted after budget) since last edit.
    pub explore_since_edit: u32,
    pub explore_budget_blocks: u32,
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

    /// Block explore thrash even when the exact string keeps changing.
    pub fn before_explore_budget(&mut self, command: &str) -> Option<ToolResult> {
        if !is_explore_shell(command) {
            return None;
        }
        if self.explore_since_edit < MAX_EXPLORE_SHELLS_SINCE_EDIT {
            return None;
        }
        self.explore_budget_blocks += 1;
        self.blocked += 1;
        Some(ToolResult {
            success: false,
            output: String::new(),
            diff: None,
            error: Some(format!(
                "Blocked: explore-shell budget exhausted ({MAX_EXPLORE_SHELLS_SINCE_EDIT} \
                 git/grep/sed/ls/… commands since the last edit). Stop archaeology. \
                 Call finish if the fix is done, or apply_patch/write_file if more changes \
                 are needed. Prefer git_status/git_diff tools or a real verify \
                 (test/lint/build) instead of more shell git probes."
            )),
            stop_reason: None,
        })
    }

    /// Call after a live explore shell actually ran.
    pub fn note_explore_executed(&mut self, command: &str) {
        if is_explore_shell(command) {
            self.explore_since_edit = self.explore_since_edit.saturating_add(1);
        }
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
        self.explore_since_edit = 0;
    }
}

/// True for archaeology-style shells (git/grep/sed/…), false for verify/build/test.
pub fn is_explore_shell(command: &str) -> bool {
    let bin = primary_binary(command);
    matches!(
        bin.as_str(),
        "git"
            | "grep"
            | "egrep"
            | "fgrep"
            | "rg"
            | "sed"
            | "awk"
            | "find"
            | "ls"
            | "cat"
            | "head"
            | "tail"
            | "wc"
            | "less"
            | "more"
            | "stat"
            | "file"
            | "echo"
            | "printf"
            | "basename"
            | "dirname"
            | "realpath"
            | "readlink"
            | "tree"
            | "nl"
            | "tac"
            | "strings"
            | "od"
            | "hexdump"
            | "which"
            | "type"
            | "pwd"
            | "env"
            | "printenv"
    )
}

fn primary_binary(command: &str) -> String {
    let mut rest = normalize_command(command);
    // Peel leading `cd … &&` / `cd … ;` so we classify the real program.
    loop {
        let lower = rest.to_ascii_lowercase();
        if !(lower.starts_with("cd ") || lower.starts_with("cd\t")) {
            break;
        }
        if let Some(idx) = rest.find("&&") {
            rest = rest[idx + 2..].trim().to_string();
            continue;
        }
        if let Some(idx) = rest.find(';') {
            rest = rest[idx + 1..].trim().to_string();
            continue;
        }
        break;
    }
    rest.split_whitespace()
        .next()
        .unwrap_or("")
        .trim_matches(|c| c == '"' || c == '\'')
        .rsplit('/')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
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
    fn classifies_git_as_explore_and_npm_as_not() {
        assert!(is_explore_shell(
            "cd frontend-v2 && git log --all --oneline -- ':(glob)**/store.ts'"
        ));
        assert!(is_explore_shell("grep -n dueDate store.ts"));
        assert!(is_explore_shell("sed -n '1,5p' a.ts"));
        assert!(!is_explore_shell("cd frontend-v2 && npm run build"));
        assert!(!is_explore_shell(
            "cd backend && .venv/bin/python manage.py test tasks"
        ));
        assert!(!is_explore_shell("npx tsc --noEmit"));
    }

    #[test]
    fn explore_budget_blocks_after_cap() {
        let mut cache = ShellCommandCache::new();
        for i in 0..MAX_EXPLORE_SHELLS_SINCE_EDIT {
            let cmd = format!("git log --oneline -{i}");
            assert!(
                cache.before_explore_budget(&cmd).is_none(),
                "should allow explore #{i}"
            );
            cache.note_explore_executed(&cmd);
        }
        let blocked = cache
            .before_explore_budget("git log --all --oneline -- store.ts")
            .unwrap();
        assert!(!blocked.success);
        assert!(blocked.error.as_ref().unwrap().contains("explore-shell budget"));
        assert_eq!(cache.explore_budget_blocks, 1);

        // Verify commands still unrestricted by explore budget.
        assert!(cache
            .before_explore_budget("cd frontend-v2 && npm run lint")
            .is_none());
    }

    #[test]
    fn clear_resets_explore_budget() {
        let mut cache = ShellCommandCache::new();
        for i in 0..MAX_EXPLORE_SHELLS_SINCE_EDIT {
            let cmd = format!("git status {i}");
            cache.note_explore_executed(&cmd);
        }
        assert!(cache.before_explore_budget("git status").is_some());
        cache.clear();
        assert!(cache.before_explore_budget("git status").is_none());
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
