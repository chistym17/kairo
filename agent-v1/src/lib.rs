pub mod api;
pub mod authorization;
pub mod governance;
pub mod cli;
pub mod config;
pub mod user_config;
pub mod llm;
pub mod logging;
pub mod loop_engine;
pub mod repo_intel;
pub mod server;
pub mod state;
pub mod tools;
pub mod verification;

pub mod benchmark;
pub mod agent;
pub mod session;
pub mod context;
pub mod chat;

// Stable embedding surface — see `api` for docs and examples.
pub use api::{
    AgentConfig, AgentEvent, AgentMode, AgentSession, ApprovalRequest, EditPolicy, EventStream,
    RunResult, RunStatus,
};
