use std::path::PathBuf;

/// All runtime configuration loaded from .env
/// One place for every config value — nothing hardcoded elsewhere.
///
/// Phase 3 will extend this with benchmark-specific config
/// (task file path, number of trials, output directory).
#[derive(Debug)]
pub struct Config {
    /// Which LLM backend to use: `groq`, `deepseek`, or `agentrouter`
    pub llm_provider: String,

    /// Groq API key — required when LLM_PROVIDER=groq
    pub groq_api_key: String,

    /// DeepSeek API key — required when LLM_PROVIDER=deepseek
    pub deepseek_api_key: String,

    /// AgentRouter API key — required when LLM_PROVIDER=agentrouter
    pub agentrouter_api_key: String,

    /// AgentRouter OpenAI-compatible base URL (…/v1)
    pub agentrouter_base_url: String,

    /// Model identifier sent to the API
    /// e.g. "llama-3.1-70b-versatile" (Groq), "deepseek-v4-flash" (DeepSeek),
    /// or "deepseek-v3.2" / "glm-4.5" (AgentRouter)
    pub model_id: String,

    /// Hard ceiling on loop iterations per run
    pub max_steps: u32,

    /// Absolute or relative path to the workspace the agent can touch.
    /// Set via `WORKSPACE_ROOT` or overridden by `--workspace` in main.
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

    /// When true, enable semantic runtime modes (EditOrJustify / ReviewOrFinish)
    /// and related tool gating. Default false — see phasev2.md Phase A.
    pub semantic_policy: bool,

    /// Edit / mutation authorization policy (Phase 12).
    /// `auto` | `confirm` | `readonly`. Default `auto` for one-shot/benchmarks;
    /// interactive sessions default to `confirm` unless overridden.
    pub edit_policy: crate::authorization::EditPolicy,
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

        let agentrouter_api_key = if llm_provider == "agentrouter" {
            require_env("AGENTROUTER_API_KEY")
        } else {
            env_or("AGENTROUTER_API_KEY", "")
        };

        let agentrouter_base_url = env_or(
            "AGENTROUTER_BASE_URL",
            "https://agentrouter.org/v1",
        );

        let default_model = match llm_provider.as_str() {
            "deepseek" => "deepseek-v4-flash",
            // Live catalog is thin; cheap DeepSeek/GLM-4.x often 503 (no channel).
            // glm-5.2 is cheapest live option; reasoning model (may use reasoning_content).
            "agentrouter" => "glm-5.2",
            _ => "llama-3.1-70b-versatile",
        };

        Self {
            llm_provider,
            groq_api_key,
            deepseek_api_key,
            agentrouter_api_key,
            agentrouter_base_url,
            model_id: env_or("MODEL_ID", default_model),
            // Ordinary tasks: 40. Raise explicitly for hard fixture experiments.
            max_steps: env_or("MAX_STEPS", "40")
                .parse::<u32>()
                .expect("MAX_STEPS must be a positive integer"),
            // Empty until set via WORKSPACE_ROOT or `--workspace` (validated in main).
            workspace_root: PathBuf::from(env_or("WORKSPACE_ROOT", "")),
            // Default 0: no artificial pacing. Rate limits use their own backoff.
            // Set STEP_DELAY_MS only if a provider needs gentle pacing.
            step_delay_ms: env_or("STEP_DELAY_MS", "0")
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
            semantic_policy: env_or("SEMANTIC_POLICY", "false")
                .parse::<bool>()
                .expect("SEMANTIC_POLICY must be true or false"),
            edit_policy: {
                let raw = env_or("EDIT_POLICY", "auto");
                crate::authorization::EditPolicy::parse(&raw).unwrap_or_else(|| {
                    panic!(
                        "EDIT_POLICY must be auto, confirm, or readonly (got {raw:?})"
                    )
                })
            },
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
