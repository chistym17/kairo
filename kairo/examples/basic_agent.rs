//! Embedding the agent as a library.
//!
//! ```bash
//! # provider credentials come from kairo/.env (LLM_PROVIDER, *_API_KEY)
//! cargo run --example basic_agent -- /path/to/repo "fix the failing tests"
//! cargo run --example basic_agent -- --ask /path/to/repo "where is the query string built?"
//! ```
//!
//! Edits pause for approval; this example approves the first one for the whole
//! task and prints every event it receives.

use std::path::PathBuf;

use kairo::api::{AgentConfig, AgentEvent, AgentMode, AgentSession, EditPolicy};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenv::dotenv().ok();

    let (mode, workspace, task) = parse_args()?;
    let config = AgentConfig::builder()
        .workspace(workspace)
        .mode(mode)
        .edit_policy(EditPolicy::Confirm)
        .build()?;

    println!(
        "[{}] {} on {} ({})",
        config.mode().as_str(),
        config.model(),
        config.workspace().display(),
        config.edit_policy().as_str()
    );

    let session = AgentSession::open(config)?;
    let mut events = session.subscribe()?;
    session.send(task).await?;

    let mut approved_for_task = false;

    while let Some(event) = events.recv().await {
        match event {
            AgentEvent::Started { run_id, .. } => println!("run {run_id} started"),

            AgentEvent::TextDelta { content } => println!("agent: {content}"),

            AgentEvent::PlanUpdated {
                version,
                objective,
                steps,
            } => {
                println!("plan v{version}: {objective}");
                for step in steps {
                    println!("  [{}] {}", if step.done { "x" } else { " " }, step.text);
                }
            }

            AgentEvent::ToolStarted { tool, .. } => println!("→ {tool}"),

            AgentEvent::ToolCompleted { tool, success, .. } => {
                println!("← {tool} {}", if success { "ok" } else { "failed" })
            }

            AgentEvent::FileChanged { path } => println!("changed {}", path.display()),

            AgentEvent::VerificationCompleted {
                command,
                success,
                summary,
            } => println!(
                "verify {command}: {} — {summary}",
                if success { "green" } else { "red" }
            ),

            AgentEvent::ApprovalRequired { request } => {
                println!("approval needed: {} ({})", request.summary, request.reason);
                // A real UI would prompt here. Approve edits once for the task,
                // and refuse anything with effects outside the workspace.
                match request.kind {
                    kairo::api::ApprovalKind::FileEdit if !approved_for_task => {
                        approved_for_task = true;
                        session.approve_for_task(request.id).await?;
                        println!("approved: edits allowed for this task");
                    }
                    kairo::api::ApprovalKind::FileEdit => {
                        session.approve(request.id).await?;
                    }
                    _ => {
                        session.reject(request.id).await?;
                        println!("rejected: effects outside the workspace");
                    }
                }
            }

            AgentEvent::Error { message } => eprintln!("error: {message}"),

            AgentEvent::Finished { result } => {
                println!("\n{:?} in {} steps", result.status, result.steps);
                if let Some(summary) = &result.summary {
                    println!("summary: {summary}");
                }
                for path in &result.changed_files {
                    println!("touched: {}", path.display());
                }
                if let Some(cost) = result.usage.estimated_cost_usd {
                    println!("cost: ${cost:.4}");
                }
                break;
            }
        }
    }

    session.shutdown()?;
    Ok(())
}

fn parse_args() -> anyhow::Result<(AgentMode, PathBuf, String)> {
    let mut args = std::env::args().skip(1).peekable();
    let mut mode = AgentMode::Agent;
    if args.peek().map(String::as_str) == Some("--ask") {
        args.next();
        mode = AgentMode::Ask;
    }

    let workspace = args
        .next()
        .map(PathBuf::from)
        .or_else(|| std::env::var("WORKSPACE_ROOT").ok().map(PathBuf::from))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "usage: basic_agent [--ask] <workspace> \"<task>\" \
                 (workspace may come from WORKSPACE_ROOT)"
            )
        })?;

    let task = args.collect::<Vec<_>>().join(" ");
    if task.trim().is_empty() {
        anyhow::bail!("usage: basic_agent [--ask] <workspace> \"<task>\"");
    }

    Ok((mode, workspace, task))
}
