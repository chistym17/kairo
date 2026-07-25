//! Local web UI (`agent-v1 serve`).
//!
//! Axum on localhost, WebSocket event stream, embedded single-page UI.

mod app;
mod static_files;

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::api::{AgentMode, EditPolicy};

pub use app::ServeOptions;

/// Bind 127.0.0.1 only and serve the UI until Ctrl-C.
pub async fn serve(options: ServeOptions) -> Result<()> {
    crate::logging::set_quiet(true);

    let app = app::build_router(options.clone())
        .await
        .context("failed to start agent session for serve")?;

    let addr = SocketAddr::from(([127, 0, 0, 1], options.port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind {addr}"))?;

    eprintln!(
        "[agent-v1] serve {} · {} · edit={} · http://{addr}",
        options.mode.as_str(),
        options.workspace.display(),
        options.edit_policy.as_str(),
    );
    eprintln!("[agent-v1] open the URL in a browser (localhost only)");

    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
            eprintln!("[agent-v1] shutting down");
        })
        .await
        .context("server error")?;

    Ok(())
}

impl ServeOptions {
    pub fn new(
        workspace: PathBuf,
        mode: AgentMode,
        edit_policy: EditPolicy,
        port: u16,
    ) -> Self {
        Self {
            workspace,
            mode,
            edit_policy,
            port,
        }
    }
}
