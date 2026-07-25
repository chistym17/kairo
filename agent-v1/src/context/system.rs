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

/// Active ask-mode prompt version (file under `prompts/`).
pub const ACTIVE_ASK_PROMPT_VERSION: &str = "ask_v1";

/// System prompt for read-only question answering ([`crate::api::AgentMode::Ask`]).
///
/// Source of truth: `prompts/prompt_ask_v1.md`.
pub fn ask_system_prompt() -> String {
    include_str!("../../prompts/prompt_ask_v1.md")
        .trim()
        .to_string()
}

pub fn compose_system_prompt(base: &str, project_instructions: Option<&str>) -> String {
    let base = crate::context::normalize::normalize_text(base);
    match project_instructions {
        Some(text) if !text.trim().is_empty() => {
            let text = crate::context::normalize::normalize_text(text);
            format!("{base}\n\n## Project instructions\n\n{text}")
        }
        _ => base,
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
        assert!(p.contains("## Communication"));
        assert!(p.contains("Prefer action over long explanations"));
        assert!(p.contains("next step must be a tool call"));
        assert!(p.contains("## Explore"));
        assert!(p.contains("## Contracts and boundaries"));
        assert!(p.contains("## Plan"));
        assert!(p.contains("## Edit"));
        assert!(p.contains("only edit code and identifiers you have observed"));
        assert!(p.contains("## Verify"));
        assert!(p.contains("## Multi-failure / broad tasks"));
        assert!(p.contains("Choose one coherent group"));
        assert!(p.contains("## Finish"));
        assert!(p.contains("Do not call `finish` while the latest verification"));
        assert!(p.contains("unfamiliar repository"));
        assert!(p.contains("understand the affected code paths before making the first patch"));
        assert!(p.contains("`locate`"));
        assert!(p.contains("what changed"));
        assert!(p.contains("remaining risks"));
        assert!(!p.contains("unified diff"));
        assert!(!p.contains("RULE 0.1"));
    }

    #[test]
    fn ask_prompt_is_read_only_and_finishes_with_an_answer() {
        let p = ask_system_prompt();
        assert!(p.contains("code-reading assistant"));
        assert!(p.contains("every write tool is denied"));
        assert!(p.contains("calling `finish` with the answer"));
        assert!(p.contains("do not attempt to apply it"));
        // Ask mode must not carry edit/verify workflow instructions.
        assert!(!p.contains("## Edit"));
        assert!(!p.contains("apply_patch` for existing files"));
    }
}
