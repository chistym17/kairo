use std::path::Path;

const AGENTS_MD: &str = "AGENTS.md";

/// Load project instructions from `AGENTS.md` in the workspace root, if present.
pub fn load_instructions(workspace_root: &Path) -> Option<String> {
    let path = workspace_root.join(AGENTS_MD);
    std::fs::read_to_string(path).ok()
}
