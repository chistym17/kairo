use std::path::PathBuf;

/// All runtime configuration loaded from .env
/// One place for every config value — nothing hardcoded elsewhere.
///
/// Phase 3 will extend this with benchmark-specific config
/// (task file path, number of trials, output directory).
#[derive(Debug)]
pub struct Config {
    /// Which LLM backend to use: `groq` or `deepseek`
    pub llm_provider: String,

    /// Groq API key — required when LLM_PROVIDER=groq
    pub groq_api_key: String,

    /// DeepSeek API key — required when LLM_PROVIDER=deepseek
    pub deepseek_api_key: String,

    /// Model identifier sent to the API
    /// e.g. "llama-3.1-70b-versatile" (Groq) or "deepseek-v4-flash" (DeepSeek)
    pub model_id: String,

    /// Hard ceiling on loop iterations per run
    pub max_steps: u32,

    /// Absolute or relative path to the workspace the agent can touch
    pub workspace_root: PathBuf,

    /// Milliseconds to wait before each LLM API call.
    pub step_delay_ms: u64,

    /// Retries when the provider rejects a malformed tool call.
    pub tool_use_max_retries: u32,

    /// Backoff between tool_use_failed retries.
    pub tool_use_retry_delay_ms: u64,

    /// Soft budget for how much context is sent to the LLM (estimated tokens).
    pub max_context_tokens: usize,

    /// Tokens reserved for the model's response / tool call formatting.
    pub context_reserve_tokens: usize,

    /// Prefer keeping at least this many most-recent session messages.
    pub keep_recent_session_messages: usize,
}

impl Config {
    /// Load config from environment variables.
    /// Call dotenv::dotenv() before this to load .env file.
    pub fn load() -> Self {
        let llm_provider = env_or("LLM_PROVIDER", "groq").to_ascii_lowercase();

        let groq_api_key = if llm_provider == "groq" {
            require_env("GROQ_API_KEY")
        } else {
            env_or("GROQ_API_KEY", "")
        };

        let deepseek_api_key = if llm_provider == "deepseek" {
            require_env("DEEPSEEK_API_KEY")
        } else {
            env_or("DEEPSEEK_API_KEY", "")
        };

        let default_model = if llm_provider == "deepseek" {
            "deepseek-v4-flash"
        } else {
            "llama-3.1-70b-versatile"
        };

        Self {
            llm_provider,
            groq_api_key,
            deepseek_api_key,
            model_id: env_or("MODEL_ID", default_model),
            max_steps: env_or("MAX_STEPS", "15")
                .parse::<u32>()
                .expect("MAX_STEPS must be a positive integer"),
            workspace_root: PathBuf::from(env_or("WORKSPACE_ROOT", "./test_repo")),
            step_delay_ms: env_or("STEP_DELAY_MS", "2000")
                .parse::<u64>()
                .expect("STEP_DELAY_MS must be a non-negative integer"),
            tool_use_max_retries: env_or("TOOL_USE_MAX_RETRIES", "3")
                .parse::<u32>()
                .expect("TOOL_USE_MAX_RETRIES must be a non-negative integer"),
            tool_use_retry_delay_ms: env_or("TOOL_USE_RETRY_DELAY_MS", "3000")
                .parse::<u64>()
                .expect("TOOL_USE_RETRY_DELAY_MS must be a non-negative integer"),
            max_context_tokens: env_or("MAX_CONTEXT_TOKENS", "16000")
                .parse::<usize>()
                .expect("MAX_CONTEXT_TOKENS must be a non-negative integer"),
            context_reserve_tokens: env_or("CONTEXT_RESERVE_TOKENS", "2048")
                .parse::<usize>()
                .expect("CONTEXT_RESERVE_TOKENS must be a non-negative integer"),
            keep_recent_session_messages: env_or("KEEP_RECENT_SESSION_MESSAGES", "20")
                .parse::<usize>()
                .expect("KEEP_RECENT_SESSION_MESSAGES must be a non-negative integer"),
        }
    }
}

fn require_env(key: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| {
        panic!(
            "missing required environment variable: {}\n\
             Make sure it is set in your .env file",
            key
        )
    })
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}
