//! User-level provider config (`~/.config/agent-v1/config.toml`).
//!
//! Priority when resolving credentials: builder/CLI → environment → this file → defaults.
//! The first-run wizard and `agent-v1 config` share the same select_* helpers.

use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

/// Persisted user settings (API key included — file mode 0600).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserConfig {
    pub provider: String,
    pub api_key: String,
    pub model: String,
}

impl UserConfig {
    pub fn is_usable(&self) -> bool {
        is_known_provider(&self.provider)
            && !self.api_key.trim().is_empty()
            && !self.model.trim().is_empty()
    }
}

pub fn is_known_provider(provider: &str) -> bool {
    matches!(
        provider.trim().to_ascii_lowercase().as_str(),
        "groq" | "deepseek" | "agentrouter"
    )
}

pub fn provider_label(provider: &str) -> &'static str {
    match provider.trim().to_ascii_lowercase().as_str() {
        "deepseek" => "DeepSeek",
        "groq" => "Groq",
        "agentrouter" => "AgentRouter",
        _ => "Unknown",
    }
}

pub fn key_env_var(provider: &str) -> &'static str {
    match provider.trim().to_ascii_lowercase().as_str() {
        "deepseek" => "DEEPSEEK_API_KEY",
        "agentrouter" => "AGENTROUTER_API_KEY",
        _ => "GROQ_API_KEY",
    }
}

pub fn default_model(provider: &str) -> &'static str {
    match provider.trim().to_ascii_lowercase().as_str() {
        "deepseek" => "deepseek-v4-flash",
        "agentrouter" => "glm-5.2",
        _ => "llama-3.1-70b-versatile",
    }
}

/// Curated models we know work. Last option in selection is always "custom".
pub fn curated_models(provider: &str) -> &'static [&'static str] {
    match provider.trim().to_ascii_lowercase().as_str() {
        "deepseek" => &["deepseek-v4-flash"],
        "agentrouter" => &["glm-5.2"],
        _ => &["llama-3.1-70b-versatile"],
    }
}

/// Mask a key for display: `sk-••••••••9f2a`.
pub fn mask_api_key(key: &str) -> String {
    let key = key.trim();
    if key.len() <= 8 {
        return "••••••••".to_string();
    }
    let suffix = &key[key.len() - 4..];
    if key.starts_with("sk-") {
        format!("sk-••••••••{suffix}")
    } else {
        let prefix_len = key.len().min(2);
        format!("{}••••••••{suffix}", &key[..prefix_len])
    }
}

/// Override path for tests: `AGENT_V1_CONFIG`. Otherwise XDG / `~/.config/agent-v1/config.toml`.
pub fn config_path() -> PathBuf {
    if let Ok(p) = std::env::var("AGENT_V1_CONFIG") {
        let p = p.trim();
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    config_dir().join("config.toml")
}

pub fn config_dir() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        let xdg = xdg.trim();
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("agent-v1");
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".config").join("agent-v1")
}

pub fn load() -> Result<Option<UserConfig>> {
    let path = config_path();
    if !path.exists() {
        return Ok(None);
    }
    let text = fs::read_to_string(&path)
        .with_context(|| format!("read {}", path.display()))?;
    let cfg: UserConfig = toml::from_str(&text)
        .with_context(|| format!("parse {}", path.display()))?;
    Ok(Some(cfg))
}

pub fn save(cfg: &UserConfig) -> Result<PathBuf> {
    if !cfg.is_usable() {
        bail!("incomplete config: need provider, api_key, and model");
    }
    let path = config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create {}", parent.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
        }
    }
    let body = toml::to_string_pretty(cfg).context("serialize user config")?;
    fs::write(&path, body).with_context(|| format!("write {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }
    Ok(path)
}

fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// True when env or user config already has a usable provider key.
pub fn credentials_ready() -> bool {
    if let Ok(Some(cfg)) = load() {
        if cfg.is_usable() {
            return true;
        }
    }
    let provider = env_nonempty("LLM_PROVIDER")
        .unwrap_or_else(|| "groq".into())
        .to_ascii_lowercase();
    env_nonempty(key_env_var(&provider)).is_some()
}

/// If credentials are missing, run the first-run wizard (when interactive) or error.
pub fn ensure_configured(interactive: bool) -> Result<()> {
    if credentials_ready() {
        return Ok(());
    }
    if !interactive {
        let path = config_path();
        bail!(
            "no LLM credentials configured.\n\
             Set GROQ_API_KEY / DEEPSEEK_API_KEY / AGENTROUTER_API_KEY (and optional LLM_PROVIDER),\n\
             or create {} with provider, api_key, and model.\n\
             Run `agent-v1` in a terminal to use the setup wizard.",
            path.display()
        );
    }
    let cfg = run_wizard()?;
    let path = save(&cfg)?;
    println!("Saved config to {}", path.display());
    println!();
    Ok(())
}

// ---------------------------------------------------------------------------
// Shared interactive selectors (first-run + `agent-v1 config`)
// ---------------------------------------------------------------------------

/// Prompt for provider: DeepSeek / Groq / AgentRouter.
pub fn select_provider() -> Result<String> {
    println!("Provider:");
    println!("  1) DeepSeek");
    println!("  2) Groq");
    println!("  3) AgentRouter");
    let choice = prompt("Choice [1]")?;
    let provider = match choice.as_str() {
        "" | "1" => "deepseek",
        "2" => "groq",
        "3" => "agentrouter",
        other if is_known_provider(other) => other,
        other => bail!("unknown provider choice: {other}"),
    };
    Ok(provider.to_string())
}

/// Prompt for a curated or custom model for `provider`.
pub fn select_model(provider: &str) -> Result<String> {
    let models = curated_models(provider);
    println!("Available tested models for {}:", provider_label(provider));
    for (i, id) in models.iter().enumerate() {
        let mark = if i == 0 { " (default)" } else { "" };
        println!("  {}) {id}{mark}", i + 1);
    }
    let custom_n = models.len() + 1;
    println!("  {custom_n}) Custom model ID");
    let choice = prompt("Choice [1]")?;
    let model = if choice.is_empty() || choice == "1" {
        models[0].to_string()
    } else if let Ok(n) = choice.parse::<usize>() {
        if (1..=models.len()).contains(&n) {
            models[n - 1].to_string()
        } else if n == custom_n {
            let custom = prompt("Model ID")?;
            if custom.trim().is_empty() {
                bail!("model ID cannot be empty");
            }
            custom
        } else {
            bail!("invalid model choice: {choice}");
        }
    } else {
        // Allow typing a model id directly.
        choice
    };
    Ok(model.trim().to_string())
}

/// Prompt for the provider's API key.
pub fn read_api_key(provider: &str) -> Result<String> {
    let api_key = prompt(&format!("API key ({})", key_env_var(provider)))?;
    if api_key.trim().is_empty() {
        bail!("API key cannot be empty");
    }
    Ok(api_key.trim().to_string())
}

fn run_wizard() -> Result<UserConfig> {
    println!();
    println!("Welcome — first-time setup for agent-v1");
    println!("Config will be saved to {}", config_path().display());
    println!();

    let provider = select_provider()?;
    println!();
    let api_key = read_api_key(&provider)?;
    println!();
    let model = select_model(&provider)?;

    Ok(UserConfig {
        provider,
        api_key,
        model,
    })
}

/// Interactive `agent-v1 config` menu: show current values and edit them.
pub fn run_config_menu() -> Result<()> {
    let mut cfg = match load()? {
        Some(c) if c.is_usable() => c,
        _ => {
            println!("No usable config yet — running setup.");
            let c = run_wizard()?;
            let path = save(&c)?;
            println!("Saved config to {}", path.display());
            c
        }
    };

    loop {
        println!();
        println!("Current configuration");
        println!();
        println!("Provider: {}", provider_label(&cfg.provider));
        println!("Model: {}", cfg.model);
        println!("API key: {}", mask_api_key(&cfg.api_key));
        println!();
        println!("1. Change provider");
        println!("2. Change model");
        println!("3. Change API key");
        println!("4. Exit");
        let choice = prompt("Select an option")?;
        println!();

        match choice.as_str() {
            "1" => {
                let provider = select_provider()?;
                println!();
                let model = select_model(&provider)?;
                println!();
                let api_key = read_api_key(&provider)?;
                cfg.provider = provider;
                cfg.model = model;
                cfg.api_key = api_key;
                save(&cfg)?;
                println!(
                    "✓ Provider updated to {}",
                    provider_label(&cfg.provider)
                );
            }
            "2" => {
                let model = select_model(&cfg.provider)?;
                cfg.model = model;
                save(&cfg)?;
                println!("✓ Model updated to {}", cfg.model);
            }
            "3" => {
                let api_key = read_api_key(&cfg.provider)?;
                cfg.api_key = api_key;
                save(&cfg)?;
                println!("✓ API key updated");
            }
            "4" | "q" | "quit" | "exit" | "" => {
                println!("Done.");
                break;
            }
            other => {
                println!("Unknown option: {other}");
            }
        }
    }
    Ok(())
}

fn prompt(label: &str) -> Result<String> {
    print!("{label}: ");
    io::stdout().flush().ok();
    let mut line = String::new();
    io::stdin()
        .read_line(&mut line)
        .context("read setup input")?;
    Ok(line.trim().to_string())
}

/// Apply user-config fallbacks into builder resolution (env wins when set).
pub fn resolve_provider(explicit: Option<String>) -> String {
    if let Some(p) = explicit {
        return p.to_ascii_lowercase();
    }
    if let Some(p) = env_nonempty("LLM_PROVIDER") {
        return p.to_ascii_lowercase();
    }
    if let Ok(Some(cfg)) = load() {
        if is_known_provider(&cfg.provider) {
            return cfg.provider.to_ascii_lowercase();
        }
    }
    "groq".to_string()
}

pub fn resolve_model(explicit: Option<String>, provider: &str) -> String {
    if let Some(m) = explicit {
        return m;
    }
    if let Some(m) = env_nonempty("MODEL_ID") {
        return m;
    }
    if let Ok(Some(cfg)) = load() {
        if cfg.provider.eq_ignore_ascii_case(provider) && !cfg.model.trim().is_empty() {
            return cfg.model;
        }
    }
    default_model(provider).to_string()
}

pub fn resolve_api_key(explicit: Option<String>, provider: &str) -> String {
    if let Some(k) = explicit {
        return k;
    }
    if let Some(k) = env_nonempty(key_env_var(provider)) {
        return k;
    }
    if let Ok(Some(cfg)) = load() {
        if cfg.provider.eq_ignore_ascii_case(provider) && !cfg.api_key.trim().is_empty() {
            return cfg.api_key;
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn save_and_load_roundtrip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::env::set_var("AGENT_V1_CONFIG", &path);

        let cfg = UserConfig {
            provider: "deepseek".into(),
            api_key: "sk-test".into(),
            model: "deepseek-v4-flash".into(),
        };
        save(&cfg).unwrap();
        let loaded = load().unwrap().unwrap();
        assert_eq!(loaded, cfg);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }

        std::env::remove_var("AGENT_V1_CONFIG");
    }

    #[test]
    fn resolve_prefers_env_over_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::env::set_var("AGENT_V1_CONFIG", &path);
        save(&UserConfig {
            provider: "deepseek".into(),
            api_key: "file-key".into(),
            model: "deepseek-v4-flash".into(),
        })
        .unwrap();

        std::env::set_var("LLM_PROVIDER", "groq");
        std::env::set_var("GROQ_API_KEY", "env-key");
        std::env::set_var("MODEL_ID", "env-model");

        assert_eq!(resolve_provider(None), "groq");
        assert_eq!(resolve_api_key(None, "groq"), "env-key");
        assert_eq!(resolve_model(None, "groq"), "env-model");

        std::env::remove_var("LLM_PROVIDER");
        std::env::remove_var("GROQ_API_KEY");
        std::env::remove_var("MODEL_ID");
        std::env::remove_var("AGENT_V1_CONFIG");
    }

    #[test]
    fn mask_api_key_hides_middle() {
        assert_eq!(mask_api_key("sk-abcdefghij9f2a"), "sk-••••••••9f2a");
        assert_eq!(mask_api_key("short"), "••••••••");
    }
}
