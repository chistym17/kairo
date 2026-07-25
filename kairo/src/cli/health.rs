//! Local readiness checks for `kairo health`.

use crate::user_config;

use super::workspace::{current_branch, detect_workspace, display_path};

/// Print a local health report. Returns the process exit code.
pub fn run() -> i32 {
    println!("Kairo health check\n");

    let mut healthy = true;
    let config_path = user_config::config_path();
    match user_config::load() {
        Ok(Some(config)) if config.is_usable() => {
            println!("✓ Config: {}", display_path(&config_path));
        }
        Ok(Some(_)) => {
            println!("✗ Config: incomplete ({})", display_path(&config_path));
            healthy = false;
        }
        Ok(None) => {
            println!("• Config: not found (checking environment)");
        }
        Err(error) => {
            println!("✗ Config: {error}");
            healthy = false;
        }
    }

    let provider = user_config::resolve_provider(None);
    if user_config::is_known_provider(&provider) {
        println!("✓ Provider: {}", user_config::provider_label(&provider));
    } else {
        println!("✗ Provider: unsupported ({provider})");
        healthy = false;
    }

    let model = user_config::resolve_model(None, &provider);
    if model.trim().is_empty() {
        println!("✗ Model: not configured");
        healthy = false;
    } else {
        println!("✓ Model: {model}");
    }

    let api_key = user_config::resolve_api_key(None, &provider);
    if api_key.trim().is_empty() {
        println!(
            "✗ API key: missing (set {} or run `kairo config`)",
            user_config::key_env_var(&provider)
        );
        healthy = false;
    } else {
        println!("✓ API key: configured");
    }

    match detect_workspace(None) {
        Ok(workspace) => {
            println!("✓ Workspace: {}", display_path(&workspace));
            match current_branch(&workspace) {
                Some(branch) => println!("✓ Git branch: {branch}"),
                None => println!("• Git: not a repository"),
            }
        }
        Err(error) => {
            println!("✗ Workspace: {error}");
            healthy = false;
        }
    }

    println!();
    if healthy {
        println!("Ready.");
        0
    } else {
        println!("Needs attention.");
        1
    }
}
