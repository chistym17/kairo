/// Active system prompt version (file under `prompts/`).
pub const ACTIVE_PROMPT_VERSION: &str = "v2";

/// Default system prompt for CLI/benchmark tasks.
///
/// Source of truth: `prompts/prompt_v2.md`. Bump by adding `prompts/prompt_vN.md`
/// and updating `ACTIVE_PROMPT_VERSION` / `include_str!` here.
pub fn default_system_prompt() -> String {
    include_str!("../../prompts/prompt_v2.md")
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
    fn default_prompt_is_v2_separated_structure() {
        assert_eq!(ACTIVE_PROMPT_VERSION, "v2");
        let p = default_system_prompt();
        assert!(p.contains("## Workflow"));
        assert!(p.contains("## General principles"));
        assert!(p.contains("## Explore"));
        assert!(p.contains("## Contracts and boundaries"));
        assert!(p.contains("## Plan"));
        assert!(p.contains("## Edit"));
        assert!(p.contains("## Verify"));
        assert!(p.contains("## Finish"));
        assert!(p.contains("unfamiliar repository"));
        assert!(p.contains("understand the affected code paths before making the first patch"));
        assert!(p.contains("`locate`"));
        assert!(p.contains("what changed"));
        assert!(p.contains("remaining risks"));
        assert!(!p.contains("unified diff"));
        assert!(!p.contains("RULE 0.1"));
    }
}
