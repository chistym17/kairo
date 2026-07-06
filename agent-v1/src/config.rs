use std::path::PathBuf;

/// All runtime configuration loaded from .env
/// One place for every config value — nothing hardcoded elsewhere.
///
/// Phase 3 will extend this with benchmark-specific config
/// (task file path, number of trials, output directory).
#[derive(Debug)]
pub struct Config {
    /// Grok API key — loaded from GROK_API_KEY in .env
    pub groq_api_key: String,

    /// Model identifier sent to the API
    /// e.g. "llama-3.1-70b-versatile" for Groq
    /// or whichever model string Grok's API uses
    pub model_id: String,

    /// Hard ceiling on loop iterations per run
    /// 15 is a good starting point for Phase 1
    pub max_steps: u32,

    /// Absolute or relative path to the workspace the agent can touch
    /// All tool operations are sandboxed inside this directory
    pub workspace_root: PathBuf,

    /// Milliseconds to wait before each LLM API call.
    /// Prevents rate limiting from rapid back-to-back requests.
    pub step_delay_ms: u64,
}

impl Config {
    /// Load config from environment variables.
    /// Call dotenv::dotenv() before this to load .env file.
    /// Panics with a clear message if any required variable is missing —
    /// better to fail fast at startup than silently use wrong values.
    pub fn load() -> Self {
        Self {
            groq_api_key: require_env("GROQ_API_KEY"),
            model_id: env_or("MODEL_ID", "llama-3.1-70b-versatile"),
            max_steps: env_or("MAX_STEPS", "15")
                .parse::<u32>()
                .expect("MAX_STEPS must be a positive integer"),
            workspace_root: PathBuf::from(env_or("WORKSPACE_ROOT", "./test_repo")),
            step_delay_ms: env_or("STEP_DELAY_MS", "2000")
                .parse::<u64>()
                .expect("STEP_DELAY_MS must be a non-negative integer"),
        }
    }
}

/// Get a required env var — panics with a clear message if missing.
fn require_env(key: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| {
        panic!(
            "missing required environment variable: {}\n\
             Make sure it is set in your .env file",
            key
        )
    })
}

/// Get an env var with a fallback default.
fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}
