/// Re-export the shared system prompt used by benchmark tasks.
pub fn system_prompt() -> String {
    crate::context::default_system_prompt()
}
