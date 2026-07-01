#[async_trait]
pub trait LlmClinet{
    async fn next_action(
        &self,
        system_prompt:&str,
        goal:&str,
        history:&[Step],
    )->Result<Action,LlmError>;

    fn model_id(&self)->&str;
}

pub enum LlmError {
    MalformedToolCall(String),
    ApiError(String),
    RateLimited,
}
