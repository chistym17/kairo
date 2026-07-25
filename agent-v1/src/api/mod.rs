//! Public API for embedding the agent.
//!
//! Everything a host application needs is here: configure a session, stream
//! events, and answer approval requests. The rest of the crate is internal
//! machinery whose shape may change.
//!
//! ```no_run
//! use agent_v1::api::{AgentConfig, AgentEvent, AgentMode, AgentSession, EditPolicy};
//!
//! # async fn run() -> anyhow::Result<()> {
//! let config = AgentConfig::builder()
//!     .workspace("/path/to/repo")
//!     .mode(AgentMode::Agent)
//!     .edit_policy(EditPolicy::Confirm)
//!     .build()?;
//!
//! let session = AgentSession::open(config)?;
//! let mut events = session.subscribe()?;
//! session.send("fix the failing tests").await?;
//!
//! while let Some(event) = events.recv().await {
//!     match event {
//!         AgentEvent::ApprovalRequired { request } => {
//!             session.approve_for_task(request.id).await?;
//!         }
//!         AgentEvent::Finished { result } => {
//!             println!("{:?}: {:?}", result.status, result.summary);
//!             break;
//!         }
//!         _ => {}
//!     }
//! }
//! # Ok(())
//! # }
//! ```

pub mod config;
pub mod events;
pub mod session;

pub use config::{AgentConfig, AgentConfigBuilder, AgentMode, EditPolicy, SessionSource};
pub use events::{
    AgentEvent, ApprovalKind, ApprovalRequest, PlanStep, RunResult, RunStatus, RunUsage,
};
pub use session::{AgentSession, EventStream};
