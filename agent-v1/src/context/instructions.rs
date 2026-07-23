use std::path::Path;

const AGENTS_MD: &str = "AGENTS.md";

use super::normalize::normalize_text;

/// Load project instructions from `AGENTS.md` in the workspace root, if present.
///
/// Text is normalized (CRLF → LF, trim trailing) so the stable system prefix
/// does not churn on editor line-ending differences.
pub fn load_instructions(workspace_root: &Path) -> Option<String> {
    let path = workspace_root.join(AGENTS_MD);
    let raw = std::fs::read_to_string(path).ok()?;
    let normalized = normalize_text(&raw);
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agents_md_crlf_matches_lf() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "line one\r\nline two\r\n").unwrap();
        let a = load_instructions(dir.path()).unwrap();

        std::fs::write(dir.path().join("AGENTS.md"), "line one\nline two\n").unwrap();
        let b = load_instructions(dir.path()).unwrap();

        assert_eq!(a, b);
        assert_eq!(a, "line one\nline two");
    }
}
