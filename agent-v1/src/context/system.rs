/// Active system prompt version (file under `prompts/`).
pub const ACTIVE_PROMPT_VERSION: &str = "v1";

/// Default system prompt for CLI/benchmark tasks.
///
/// Source of truth: `prompts/prompt_v1.md`. Bump the version by adding
/// `prompts/prompt_vN.md` and pointing `ACTIVE_PROMPT_VERSION` / `include_str!` here.
pub fn default_system_prompt() -> String {
    include_str!("../../prompts/prompt_v1.md")
        .trim()
        .to_string()
}

pub fn compose_system_prompt(base: &str, project_instructions: Option<&str>) -> String {
    match project_instructions {
        Some(text) if !text.trim().is_empty() => {
            format!("{base}\n\n## Project instructions\n\n{text}")
        }
        _ => base.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_prompt_is_v1_workflow_style() {
        assert_eq!(ACTIVE_PROMPT_VERSION, "v1");
        let p = default_system_prompt();
        assert!(p.contains("## Workflow"));
        assert!(p.contains("## Explore"));
        assert!(p.contains("## Plan"));
        assert!(p.contains("## Edit"));
        assert!(p.contains("## Verify"));
        assert!(p.contains("## Finish"));
        assert!(p.contains("Batch independent read-only tools"));
        assert!(p.contains("Prefer one minimal correct patch"));
        assert!(!p.contains("RULE 0.1"));
    }
}
