use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

use crate::authorization::EditPolicy as InternalEditPolicy;
use crate::config::Config;

/// How much authority the agent has over the workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentMode {
    /// Read-only. Explores and answers; every mutation is denied by the harness.
    Ask,
    /// Explores, edits, and verifies under an [`EditPolicy`].
    Agent,
}

impl AgentMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Agent => "agent",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "ask" => Some(Self::Ask),
            "agent" => Some(Self::Agent),
            _ => None,
        }
    }

    /// Policy applied when the caller does not choose one.
    pub fn default_edit_policy(self) -> EditPolicy {
        match self {
            Self::Ask => EditPolicy::ReadOnly,
            Self::Agent => EditPolicy::Confirm,
        }
    }
}

/// What the harness permits without asking the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditPolicy {
    /// Deny every mutation and external side effect.
    ReadOnly,
    /// Pause for approval before the first edit and before external side effects.
    Confirm,
    /// Apply edits without asking.
    Auto,
}

impl EditPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "readonly",
            Self::Confirm => "confirm",
            Self::Auto => "auto",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        InternalEditPolicy::parse(s).map(Self::from)
    }
}

impl From<InternalEditPolicy> for EditPolicy {
    fn from(value: InternalEditPolicy) -> Self {
        match value {
            InternalEditPolicy::Auto => Self::Auto,
            InternalEditPolicy::ConfirmBeforeFirstEdit => Self::Confirm,
            InternalEditPolicy::ReadOnly => Self::ReadOnly,
        }
    }
}

impl From<EditPolicy> for InternalEditPolicy {
    fn from(value: EditPolicy) -> Self {
        match value {
            EditPolicy::Auto => Self::Auto,
            EditPolicy::Confirm => Self::ConfirmBeforeFirstEdit,
            EditPolicy::ReadOnly => Self::ReadOnly,
        }
    }
}

/// Where session memory (messages, plan, decisions) lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionSource {
    /// In-memory only: memory persists across `send` calls, nothing is written to disk.
    Ephemeral,
    /// Persisted to the sessions directory under a new id.
    Persistent,
    /// Resume a persisted session by id.
    Resume(String),
}

/// Everything needed to open an [`crate::api::AgentSession`].
///
/// Build with [`AgentConfig::builder`]. Provider credentials come from the
/// builder, then environment (`LLM_PROVIDER`, `*_API_KEY`, `MODEL_ID`), then
/// `~/.config/agent-v1/config.toml`.
#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub(crate) workspace: PathBuf,
    pub(crate) mode: AgentMode,
    pub(crate) edit_policy: EditPolicy,
    pub(crate) session_source: SessionSource,
    pub(crate) system_prompt: String,
    pub(crate) approval_timeout: std::time::Duration,
    pub(crate) inner: Config,
}

impl AgentConfig {
    pub fn builder() -> AgentConfigBuilder {
        AgentConfigBuilder::default()
    }

    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    pub fn mode(&self) -> AgentMode {
        self.mode
    }

    pub fn edit_policy(&self) -> EditPolicy {
        self.edit_policy
    }

    pub fn model(&self) -> &str {
        &self.inner.model_id
    }

    pub fn provider(&self) -> &str {
        &self.inner.llm_provider
    }

    pub fn max_steps(&self) -> u32 {
        self.inner.max_steps
    }
}

/// Incremental builder for [`AgentConfig`].
#[derive(Debug, Default, Clone)]
pub struct AgentConfigBuilder {
    workspace: Option<PathBuf>,
    mode: Option<AgentMode>,
    edit_policy: Option<EditPolicy>,
    session_source: Option<SessionSource>,
    system_prompt: Option<String>,
    provider: Option<String>,
    model: Option<String>,
    api_key: Option<String>,
    base_url: Option<String>,
    max_steps: Option<u32>,
    approval_timeout: Option<std::time::Duration>,
}

impl AgentConfigBuilder {
    /// Repository the agent may read and (in [`AgentMode::Agent`]) edit. Required.
    pub fn workspace(mut self, path: impl Into<PathBuf>) -> Self {
        self.workspace = Some(path.into());
        self
    }

    pub fn mode(mut self, mode: AgentMode) -> Self {
        self.mode = Some(mode);
        self
    }

    /// Ignored in [`AgentMode::Ask`], which is always read-only.
    pub fn edit_policy(mut self, policy: EditPolicy) -> Self {
        self.edit_policy = Some(policy);
        self
    }

    pub fn session(mut self, source: SessionSource) -> Self {
        self.session_source = Some(source);
        self
    }

    /// Replace the built-in prompt for this session.
    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    /// `groq` | `deepseek` | `agentrouter`. Defaults to `LLM_PROVIDER`.
    pub fn provider(mut self, provider: impl Into<String>) -> Self {
        self.provider = Some(provider.into());
        self
    }

    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Key for the selected provider. Defaults to the provider's `*_API_KEY` variable.
    pub fn api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }

    /// OpenAI-compatible base URL (agentrouter only).
    pub fn base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = Some(url.into());
        self
    }

    pub fn max_steps(mut self, steps: u32) -> Self {
        self.max_steps = Some(steps);
        self
    }

    /// How long an [`crate::api::AgentEvent::ApprovalRequired`] waits before
    /// being auto-rejected. Default 10 minutes.
    pub fn approval_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.approval_timeout = Some(timeout);
        self
    }

    /// Validate the workspace and resolve provider credentials.
    pub fn build(self) -> Result<AgentConfig> {
        let Some(workspace) = self.workspace else {
            bail!("workspace is required: AgentConfig::builder().workspace(path)");
        };
        if !workspace.exists() {
            bail!("workspace does not exist: {}", workspace.display());
        }
        if !workspace.is_dir() {
            bail!("workspace is not a directory: {}", workspace.display());
        }
        let workspace = workspace.canonicalize().unwrap_or(workspace);

        let mode = self.mode.unwrap_or(AgentMode::Agent);
        // Ask mode is read-only by construction — an explicit policy cannot widen it.
        let edit_policy = match mode {
            AgentMode::Ask => EditPolicy::ReadOnly,
            AgentMode::Agent => self.edit_policy.unwrap_or(mode.default_edit_policy()),
        };

        let provider = crate::user_config::resolve_provider(self.provider);
        if !crate::user_config::is_known_provider(&provider) {
            bail!(
                "unknown provider {provider:?}: use \"groq\", \"deepseek\", or \"agentrouter\""
            );
        }
        let model = crate::user_config::resolve_model(self.model, &provider);
        let key_var = crate::user_config::key_env_var(&provider);
        let api_key = crate::user_config::resolve_api_key(self.api_key, &provider);
        if api_key.trim().is_empty() {
            bail!(
                "missing API key for provider {provider}. \
                 Set {key_var}, or run agent-v1 once to save ~/.config/agent-v1/config.toml"
            );
        }

        let base_url = self
            .base_url
            .unwrap_or_else(|| env_or("AGENTROUTER_BASE_URL", "https://agentrouter.org/v1"));

        let system_prompt = self.system_prompt.unwrap_or_else(|| match mode {
            AgentMode::Ask => crate::context::ask_system_prompt(),
            AgentMode::Agent => crate::context::default_system_prompt(),
        });

        let inner = Config {
            llm_provider: provider.clone(),
            groq_api_key: if provider == "groq" {
                api_key.clone()
            } else {
                String::new()
            },
            deepseek_api_key: if provider == "deepseek" {
                api_key.clone()
            } else {
                String::new()
            },
            agentrouter_api_key: if provider == "agentrouter" {
                api_key.clone()
            } else {
                String::new()
            },
            agentrouter_base_url: base_url,
            model_id: model,
            max_steps: self
                .max_steps
                .unwrap_or_else(|| env_parse("MAX_STEPS", 40)),
            workspace_root: workspace.clone(),
            step_delay_ms: env_parse("STEP_DELAY_MS", 0),
            tool_use_max_retries: env_parse("TOOL_USE_MAX_RETRIES", 3),
            tool_use_retry_delay_ms: env_parse("TOOL_USE_RETRY_DELAY_MS", 3000),
            max_context_tokens: env_parse("MAX_CONTEXT_TOKENS", 16000),
            context_reserve_tokens: env_parse("CONTEXT_RESERVE_TOKENS", 2048),
            keep_recent_session_messages: env_parse("KEEP_RECENT_SESSION_MESSAGES", 20),
            semantic_policy: env_parse("SEMANTIC_POLICY", false),
            edit_policy: edit_policy.into(),
        };

        Ok(AgentConfig {
            workspace,
            mode,
            edit_policy,
            session_source: self.session_source.unwrap_or(SessionSource::Ephemeral),
            system_prompt,
            approval_timeout: self
                .approval_timeout
                .unwrap_or(std::time::Duration::from_secs(600)),
            inner,
        })
    }
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
}

/// Malformed values fall back to the default rather than panicking — a library
/// must not abort the host process over an environment typo.
fn env_parse<T: std::str::FromStr>(key: &str, default: T) -> T {
    std::env::var(key)
        .ok()
        .and_then(|v| v.trim().parse::<T>().ok())
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn base(dir: &Path) -> AgentConfigBuilder {
        AgentConfig::builder()
            .workspace(dir)
            .provider("deepseek")
            .api_key("test-key")
            .model("test-model")
    }

    #[test]
    fn agent_mode_defaults_to_confirm() {
        let dir = tempdir().unwrap();
        let config = base(dir.path()).mode(AgentMode::Agent).build().unwrap();
        assert_eq!(config.edit_policy(), EditPolicy::Confirm);
        assert_eq!(config.inner.edit_policy, InternalEditPolicy::ConfirmBeforeFirstEdit);
    }

    #[test]
    fn ask_mode_is_read_only_even_when_auto_requested() {
        let dir = tempdir().unwrap();
        let config = base(dir.path())
            .mode(AgentMode::Ask)
            .edit_policy(EditPolicy::Auto)
            .build()
            .unwrap();
        assert_eq!(config.edit_policy(), EditPolicy::ReadOnly);
        assert_eq!(config.inner.edit_policy, InternalEditPolicy::ReadOnly);
        assert!(config.system_prompt.contains("code-reading assistant"));
    }

    #[test]
    fn agent_mode_uses_the_coding_prompt() {
        let dir = tempdir().unwrap();
        let config = base(dir.path()).build().unwrap();
        assert_eq!(config.mode(), AgentMode::Agent);
        assert!(config.system_prompt.contains("autonomous coding agent"));
    }

    #[test]
    fn missing_workspace_is_an_error_not_a_panic() {
        let err = AgentConfig::builder()
            .provider("deepseek")
            .api_key("k")
            .build()
            .unwrap_err();
        assert!(err.to_string().contains("workspace is required"));

        let err = AgentConfig::builder()
            .workspace("/nonexistent/path/for/tests")
            .provider("deepseek")
            .api_key("k")
            .build()
            .unwrap_err();
        assert!(err.to_string().contains("does not exist"));
    }

    #[test]
    fn unknown_provider_is_rejected() {
        let dir = tempdir().unwrap();
        let err = AgentConfig::builder()
            .workspace(dir.path())
            .provider("openai")
            .build()
            .unwrap_err();
        assert!(err.to_string().contains("unknown provider"));
    }

    #[test]
    fn explicit_overrides_beat_the_environment() {
        let dir = tempdir().unwrap();
        let config = base(dir.path()).max_steps(7).build().unwrap();
        assert_eq!(config.max_steps(), 7);
        assert_eq!(config.model(), "test-model");
        assert_eq!(config.provider(), "deepseek");
        assert_eq!(config.inner.deepseek_api_key, "test-key");
    }

    #[test]
    fn policy_round_trips_through_internal_representation() {
        for policy in [EditPolicy::ReadOnly, EditPolicy::Confirm, EditPolicy::Auto] {
            let internal: InternalEditPolicy = policy.into();
            assert_eq!(EditPolicy::from(internal), policy);
            assert_eq!(EditPolicy::parse(policy.as_str()), Some(policy));
        }
    }
}
