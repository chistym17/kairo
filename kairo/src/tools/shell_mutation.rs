//! Detect shell commands that mutate the workspace.
//!
//! Phase 12 v1: **block** these and require `apply_patch` / `write_file`.
//! Detection is intentionally pattern-based (not a full shell parser).

/// True when the command likely creates, overwrites, moves, or deletes files.
pub fn is_shell_workspace_mutation(command: &str) -> bool {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();

    if has_inplace_sed(&lower) || has_inplace_perl(&lower) {
        return true;
    }
    if has_file_redirect(trimmed) {
        return true;
    }
    if has_heredoc_write(&lower) {
        return true;
    }
    if looks_like_language_file_write(&lower) {
        return true;
    }

    for segment in split_shell_segments(trimmed) {
        let bin = primary_binary(segment);
        if is_mutating_binary(&bin, segment) {
            return true;
        }
    }
    false
}

/// Denial message for blocked shell mutations.
pub fn shell_mutation_blocked_message(command: &str) -> String {
    format!(
        "Blocked: shell workspace mutation is not allowed via run_shell/run_command.\n\
         Command: {command}\n\
         Use apply_patch (existing files) or write_file (new files) instead.\n\
         Read-only shells (cat/head/grep/git status/diff/log, tests) remain allowed."
    )
}

fn token_is_sed(t: &str) -> bool {
    t == "sed" || t.ends_with("/sed")
}

fn token_is_perl(t: &str) -> bool {
    t == "perl" || t.ends_with("/perl")
}

fn has_inplace_sed(lower: &str) -> bool {
    let tokens: Vec<&str> = lower.split_whitespace().collect();
    for (i, t) in tokens.iter().enumerate() {
        if !token_is_sed(t) {
            continue;
        }
        for u in tokens.iter().skip(i + 1) {
            if *u == "--" {
                break;
            }
            if *u == "--in-place" || u.starts_with("--in-place=") {
                return true;
            }
            // -i / -i.bak / -ibak
            if *u == "-i" || (u.starts_with("-i") && u.len() > 2) {
                return true;
            }
            // Combined short options containing i (e.g. -ri), excluding lone -n/-e/-E/-r
            if u.starts_with('-') && !u.starts_with("--") {
                let flags: String = u
                    .trim_start_matches('-')
                    .chars()
                    .take_while(|c| c.is_ascii_alphabetic())
                    .collect();
                if flags.contains('i') {
                    return true;
                }
            }
        }
    }
    false
}

fn has_inplace_perl(lower: &str) -> bool {
    let tokens: Vec<&str> = lower.split_whitespace().collect();
    for (i, t) in tokens.iter().enumerate() {
        if !token_is_perl(t) {
            continue;
        }
        for u in tokens.iter().skip(i + 1) {
            if *u == "-e" || *u == "-E" || *u == "--" {
                break;
            }
            if *u == "-i"
                || u.starts_with("-i")
                || *u == "-pi"
                || u.starts_with("-pi")
                || (u.starts_with('-') && !u.starts_with("--") && u.contains('i'))
            {
                return true;
            }
        }
    }
    false
}

/// `>` / `>>` to a real path (not `/dev/null`, not `>&`).
fn has_file_redirect(cmd: &str) -> bool {
    let bytes = cmd.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'>' {
            let mut j = i;
            if j + 1 < bytes.len() && bytes[j + 1] == b'>' {
                j += 1;
            }
            let after = j + 1;
            let rest = cmd[after..].trim_start();
            if rest.starts_with('&') {
                i += 1;
                continue;
            }
            let target = rest
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim_matches(|c| c == '"' || c == '\'');
            if !target.is_empty() && target != "/dev/null" && !target.eq_ignore_ascii_case("nul") {
                return true;
            }
        }
        i += 1;
    }
    false
}

fn has_heredoc_write(lower: &str) -> bool {
    (lower.contains("<<") || lower.contains("<<-"))
        && (lower.contains('>') || lower.contains(" tee "))
}

fn looks_like_language_file_write(lower: &str) -> bool {
    let has_interp =
        lower.contains("python") || lower.contains("node ") || lower.contains("node\t") || lower.contains("ruby");
    if !has_interp {
        return false;
    }
    let write_open = lower.contains("open(")
        && (lower.contains("'w'")
            || lower.contains("\"w\"")
            || lower.contains("'a'")
            || lower.contains("\"a\"")
            || lower.contains("'w+'")
            || lower.contains("\"w+\""));
    let path_write = lower.contains("write_text(")
        || lower.contains("write_bytes(")
        || lower.contains("fs.write")
        || lower.contains("writefilesync")
        || lower.contains("writefile(");
    write_open || path_write
}

fn is_mutating_binary(bin: &str, segment: &str) -> bool {
    match bin {
        "cp" | "mv" | "rm" | "rmdir" | "unlink" | "truncate" | "tee" | "install" | "chmod"
        | "chown" | "ln" | "touch" | "mkdir" | "dd" | "rsync" => true,
        "git" => {
            let sub = second_token(segment).unwrap_or("");
            matches!(
                sub,
                "checkout"
                    | "restore"
                    | "reset"
                    | "clean"
                    | "stash"
                    | "apply"
                    | "am"
                    | "cherry-pick"
                    | "rebase"
                    | "merge"
                    | "revert"
                    | "rm"
                    | "add"
                    | "commit"
                    | "mv"
                    | "update-index"
            )
        }
        _ => false,
    }
}

/// Split on `|`, `&&`, and `;` so each simple command can be classified.
pub(crate) fn split_shell_segments(command: &str) -> Vec<&str> {
    let mut out = Vec::new();
    for pipe_part in command.split('|') {
        for and_part in pipe_part.split("&&") {
            for semi in and_part.split(';') {
                let s = semi.trim();
                if !s.is_empty() {
                    out.push(s);
                }
            }
        }
    }
    if out.is_empty() {
        out.push(command);
    }
    out
}

/// Leading program name of a simple command (after peeling `cd … &&` / `cd … ;`).
pub(crate) fn primary_binary(command: &str) -> String {
    let mut rest = command.trim().to_string();
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

fn second_token(command: &str) -> Option<&str> {
    let mut rest = command.trim();
    let lower = rest.to_ascii_lowercase();
    if lower.starts_with("cd ") {
        if let Some(idx) = rest.find("&&") {
            rest = rest[idx + 2..].trim();
        }
    }
    let mut parts = rest.split_whitespace();
    let _bin = parts.next()?;
    parts.next()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_sed_inplace_allows_sed_print() {
        assert!(is_shell_workspace_mutation(
            "sed -i 's|/tasks/?due_date=|/tasks/?date=|' frontend-v2/src/lib/api.ts"
        ));
        assert!(is_shell_workspace_mutation("sed -i.bak s/a/b/ file.ts"));
        assert!(!is_shell_workspace_mutation(
            "sed -n '190,200p' frontend-v2/src/lib/api.ts"
        ));
        assert!(!is_shell_workspace_mutation("sed -n '194p' a.ts"));
    }

    #[test]
    fn blocks_redirects_and_git_mutating() {
        assert!(is_shell_workspace_mutation("echo hi > out.txt"));
        assert!(is_shell_workspace_mutation("echo hi >> out.txt"));
        assert!(!is_shell_workspace_mutation("npm test >/dev/null"));
        assert!(!is_shell_workspace_mutation("npm test 2>/dev/null"));
        assert!(is_shell_workspace_mutation("git checkout -- api.ts"));
        assert!(is_shell_workspace_mutation("git restore api.ts"));
        assert!(!is_shell_workspace_mutation("git status"));
        assert!(!is_shell_workspace_mutation(
            "git diff frontend-v2/src/lib/api.ts"
        ));
        assert!(!is_shell_workspace_mutation("git log --oneline -5"));
    }

    #[test]
    fn allows_verify_commands() {
        assert!(!is_shell_workspace_mutation("npm run build"));
        assert!(!is_shell_workspace_mutation("npm test"));
        assert!(!is_shell_workspace_mutation("cargo test"));
        assert!(!is_shell_workspace_mutation(
            "cd backend && .venv/bin/python manage.py test tasks"
        ));
        assert!(!is_shell_workspace_mutation("pytest"));
        assert!(!is_shell_workspace_mutation("cat api.ts | head -20"));
    }

    #[test]
    fn blocks_cp_rm_tee() {
        assert!(is_shell_workspace_mutation("cp a.ts b.ts"));
        assert!(is_shell_workspace_mutation("rm -f a.ts"));
        assert!(is_shell_workspace_mutation("tee out.txt"));
    }
}
