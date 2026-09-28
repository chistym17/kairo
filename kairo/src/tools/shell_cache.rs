use std::collections::HashMap;
use std::path::Path;

use crate::tools::ToolResult;

/// Max times the same non-verify command may be returned without an intervening edit:
/// 1 = real execution, 2 = cached replay, 3+ = blocked.
pub const MAX_IDENTICAL_SHELL_WITHOUT_EDIT: u32 = 2;

/// Verify commands (tests/lint/build) get no soft cache replay: second identical
/// attempt without an intervening edit is denied immediately.
pub const MAX_IDENTICAL_VERIFY_WITHOUT_EDIT: u32 = 1;

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
        let verify = is_verify_shell(command);

        // Verify: no soft replay — identical re-run without a mutation is denied.
        let allow_soft = if verify {
            entry.times_seen <= MAX_IDENTICAL_VERIFY_WITHOUT_EDIT
        } else {
            entry.times_seen <= MAX_IDENTICAL_SHELL_WITHOUT_EDIT
        };

        if allow_soft {
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
            let hint = if verify {
                "Verification already ran with this exact command and nothing has changed since. \
                 Do not re-verify identically. Apply a patch (or a narrower diagnostic), then verify again."
            } else {
                "Do not retry it. Call finish if the task is done, or apply a new edit / different command."
            };
            Some(ToolResult {
                success: false,
                output: String::new(),
                diff: None,
                error: Some(format!(
                    "Blocked: identical {} already returned {} times since the last edit \
                     (first at step {}). {hint}",
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

/// True for test/lint/build-style commands that should not be re-run identically
/// without an intervening edit.
pub fn is_verify_shell(command: &str) -> bool {
    let c = normalize_command(command).to_ascii_lowercase();
    if c.contains("npm test")
        || c.contains("npm run test")
        || c.contains("yarn test")
        || c.contains("pnpm test")
        || c.contains("cargo test")
        || c.contains("cargo check")
        || c.contains("cargo build")
        || c.contains("pytest")
        || c.contains("node --test")
        || c.contains("npx tsc")
        || c.contains("npm run lint")
        || c.contains("npm run build")
        || c.contains("make test")
        || c.contains("make check")
    {
        return true;
    }
    let bin = primary_binary(command);
    match bin.as_str() {
        "pytest" | "jest" | "vitest" | "mocha" | "phpunit" => true,
        "go" => c.contains(" test"),
        _ => false,
    }
}

/// True for archaeology-style shells (git/grep/sed/…), false for verify/build/test.
///
/// Every `|` / `&&` / `;` segment must be explore-safe. Git is explore only for
/// known read-only subcommands (`status`/`log`/`diff`/…), not `push`/`fetch`/….
pub fn is_explore_shell(command: &str) -> bool {
    use crate::tools::shell_mutation::split_shell_segments;

    let segments = split_shell_segments(command);
    if segments.is_empty() {
        return false;
    }
    segments.iter().all(|segment| is_explore_segment(segment))
}

fn is_explore_segment(segment: &str) -> bool {
    use crate::tools::shell_mutation::primary_binary;

    let bin = primary_binary(segment);
    if bin.is_empty() {
        return false;
    }
    // Pure `cd` is neutral in a chain.
    if bin == "cd" {
        return true;
    }
    if bin == "git" {
        return is_explore_git_segment(segment);
    }
    matches!(
        bin.as_str(),
        "grep"
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
            | "true"
            | "false"
    )
}

fn is_explore_git_segment(segment: &str) -> bool {
    let sub = git_subcommand(segment);
    match sub.as_str() {
        "status" | "log" | "diff" | "show" | "blame" | "rev-parse" | "ls-files" | "ls-tree"
        | "cat-file" | "describe" | "shortlog" | "name-rev" | "whatchanged" | "range-diff"
        | "version" | "help" | "grep" => true,
        "branch" => !git_branch_mutates(segment),
        "remote" => is_readonly_git_remote(segment),
        _ => false,
    }
}

fn git_subcommand(segment: &str) -> String {
    let mut rest = segment.trim();
    // Peel leading cd (should already be its own segment, but be defensive).
    let lower = rest.to_ascii_lowercase();
    if lower.starts_with("cd ") || lower.starts_with("cd\t") {
        if let Some(idx) = rest.find("&&") {
            rest = rest[idx + 2..].trim();
        } else if let Some(idx) = rest.find(';') {
            rest = rest[idx + 1..].trim();
        }
    }
    let mut parts = rest.split_whitespace();
    let Some(first) = parts.next() else {
        return String::new();
    };
    let base = first
        .trim_matches(|c| c == '"' || c == '\'')
        .rsplit('/')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    if base != "git" {
        return String::new();
    }
    // Skip common global flags before the subcommand.
    while let Some(tok) = parts.next() {
        let t = tok.trim_matches(|c| c == '"' || c == '\'');
        if t == "-C" || t == "-c" {
            let _ = parts.next();
            continue;
        }
        if t.starts_with('-') {
            continue;
        }
        return t.to_ascii_lowercase();
    }
    String::new()
}

fn git_branch_mutates(segment: &str) -> bool {
    let mut parts = segment.split_whitespace();
    // Advance past git (+ globals) to the `branch` subcommand.
    while let Some(tok) = parts.next() {
        let t = tok
            .trim_matches(|c| c == '"' || c == '\'')
            .rsplit('/')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        if t == "git" {
            continue;
        }
        if t.starts_with('-') {
            if t == "-c" || t == "-C" {
                let _ = parts.next();
            }
            continue;
        }
        if t != "branch" {
            return false;
        }
        break;
    }
    parts.any(|tok| {
        matches!(
            tok.trim_matches(|c| c == '"' || c == '\''),
            "-d" | "-D"
                | "-m"
                | "-M"
                | "-c"
                | "-C"
                | "--delete"
                | "--move"
                | "--copy"
                | "--unset-upstream"
        )
    })
}

fn is_readonly_git_remote(segment: &str) -> bool {
    let mut parts = segment.split_whitespace();
    // Advance to `remote`
    while let Some(tok) = parts.next() {
        let t = tok
            .trim_matches(|c| c == '"' || c == '\'')
            .rsplit('/')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        if t == "git" {
            continue;
        }
        if t.starts_with('-') {
            if t == "-c" || t == "-C" {
                let _ = parts.next();
            }
            continue;
        }
        if t != "remote" {
            return false;
        }
        break;
    }
    let args: Vec<&str> = parts
        .map(|t| t.trim_matches(|c| c == '"' || c == '\''))
        .collect();
    if args.is_empty() {
        return true;
    }
    matches!(args[0], "-v" | "--verbose" | "show" | "get-url")
}

fn primary_binary(command: &str) -> String {
    crate::tools::shell_mutation::primary_binary(command)
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
        assert!(is_explore_shell("git status"));
        assert!(is_explore_shell("git remote -v"));
        assert!(!is_explore_shell("git push origin main"));
        assert!(!is_explore_shell("git fetch origin"));
        assert!(!is_explore_shell("git pull"));
        assert!(!is_explore_shell("git status && git push origin main"));
        assert!(!is_explore_shell("cd frontend-v2 && npm run build"));
        assert!(!is_explore_shell(
            "cd backend && .venv/bin/python manage.py test tasks"
        ));
        assert!(!is_explore_shell("npx tsc --noEmit"));
    }

    #[test]
    fn classifies_verify_shells() {
        assert!(is_verify_shell("npm test"));
        assert!(is_verify_shell("cd fixture && npm test"));
        assert!(is_verify_shell("cargo test --lib"));
        assert!(is_verify_shell("npx tsc --noEmit"));
        assert!(!is_verify_shell("git status"));
        assert!(!is_verify_shell("echo hello"));
    }

    #[test]
    fn identical_verify_is_blocked_without_soft_replay() {
        let mut cache = ShellCommandCache::new();
        let cwd = PathBuf::from(".");
        let cmd = "npm test";

        cache.store_live("run_shell", cmd, &cwd, 3, &ok("fail 10"));

        let blocked = cache.before_execute("run_shell", cmd, &cwd).unwrap();
        assert!(!blocked.success);
        assert!(blocked
            .error
            .as_ref()
            .unwrap()
            .contains("Verification already ran"));
        assert_eq!(cache.blocked, 1);
        assert_eq!(cache.cache_hits, 0);
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
