use std::io::{self, Write};
use std::path::PathBuf;

use anyhow::{bail, Context, Result};

use crate::api::{AgentConfig, AgentEvent, AgentMode, AgentSession, EditPolicy, RunResult, SessionSource};
use crate::logging;

use super::args::{CliArgs, Command, OutputFormat, SessionArg};
use super::output::{exit_code, EventSink};
use super::print_usage;

pub async fn run(cli: CliArgs) -> Result<i32> {
    if matches!(cli.command, Command::Serve) {
        return run_serve(cli).await;
    }

    if matches!(cli.output, OutputFormat::Jsonl) {
        logging::set_quiet(true);
    }

    let workspace = resolve_workspace(cli.workspace.clone())?;
    let mode = cli.mode();
    let edit_policy = match mode {
        AgentMode::Ask => EditPolicy::ReadOnly,
        AgentMode::Agent => cli
            .edit_policy
            .unwrap_or_else(|| mode.default_edit_policy()),
    };

    let session_source = match &cli.session {
        SessionArg::New => SessionSource::Persistent,
        SessionArg::Ephemeral => SessionSource::Ephemeral,
        SessionArg::Resume(id) => SessionSource::Resume(id.clone()),
    };

    let config = AgentConfig::builder()
        .workspace(&workspace)
        .mode(mode)
        .edit_policy(edit_policy)
        .session(session_source)
        .build()
        .context("invalid agent config")?;

    let sink = EventSink::new(cli.output);
    sink.banner(
        config.mode().as_str(),
        config.model(),
        config.workspace(),
        config.edit_policy(),
    );
    sink.info(&format!("session policy: {}", session_label(&cli.session)));

    let session = AgentSession::open(config).context("failed to open session")?;
    sink.info(&format!("session: {}", session.session_id()));

    let mut sink = sink;
    let code = if let Some(task) = cli.task {
        run_oneshot(&session, &mut sink, task).await?
    } else {
        run_interactive(&session, &mut sink).await?
    };

    session.shutdown().context("failed to shut down session")?;
    Ok(code)
}

async fn run_serve(cli: CliArgs) -> Result<i32> {
    let workspace = resolve_workspace(cli.workspace.clone())?;
    let mode = AgentMode::Agent;
    let edit_policy = cli
        .edit_policy
        .unwrap_or(EditPolicy::Confirm);

    crate::server::serve(crate::server::ServeOptions::new(
        workspace,
        mode,
        edit_policy,
        cli.port,
    ))
    .await?;
    Ok(0)
}

async fn run_oneshot(
    session: &AgentSession,
    sink: &mut EventSink,
    task: String,
) -> Result<i32> {
    let mut events = session.subscribe().context("event stream")?;
    session.send(task).await?;
    let result = drain_turn(session, &mut events, sink).await?;
    Ok(exit_code(&result))
}

async fn run_interactive(session: &AgentSession, sink: &mut EventSink) -> Result<i32> {
    let mut events = session.subscribe().context("event stream")?;
    sink.info("interactive — type a task, /exit to quit");

    let mut last_code = 0;
    loop {
        print!("\n> ");
        let _ = io::stdout().flush();
        let Some(line) = read_line().await? else {
            sink.info("EOF — shutting down");
            break;
        };
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }
        if matches!(line.as_str(), "/exit" | "/quit" | "exit" | "quit") {
            sink.info("exiting");
            break;
        }
        if line == "/help" {
            eprintln!("commands: /exit  /help");
            eprintln!("type any other line to send it as a task");
            continue;
        }

        session.send(line).await?;
        let result = drain_turn(session, &mut events, sink).await?;
        last_code = exit_code(&result);
    }
    Ok(last_code)
}

async fn drain_turn(
    session: &AgentSession,
    events: &mut crate::api::EventStream,
    sink: &mut EventSink,
) -> Result<RunResult> {
    loop {
        let event = events
            .recv()
            .await
            .context("event stream closed before the turn finished")?;

        let approval = match &event {
            AgentEvent::ApprovalRequired { request } => Some(request.clone()),
            _ => None,
        };

        sink.emit(&event);

        if let Some(request) = approval {
            sink.handle_approval(session, request).await?;
        }

        if let AgentEvent::Finished { result } = event {
            return Ok(result);
        }
    }
}

fn resolve_workspace(cli_workspace: Option<PathBuf>) -> Result<PathBuf> {
    let raw = cli_workspace
        .or_else(|| {
            std::env::var("WORKSPACE_ROOT")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .map(PathBuf::from)
        })
        .ok_or_else(|| {
            anyhow::anyhow!(
                "workspace not set — pass --workspace <path> or set WORKSPACE_ROOT in .env"
            )
        })?;

    if !raw.exists() {
        bail!("workspace does not exist: {}", raw.display());
    }
    if !raw.is_dir() {
        bail!("workspace is not a directory: {}", raw.display());
    }
    Ok(raw.canonicalize().unwrap_or(raw))
}

fn session_label(arg: &SessionArg) -> String {
    match arg {
        SessionArg::New => "new (persistent)".into(),
        SessionArg::Ephemeral => "ephemeral".into(),
        SessionArg::Resume(id) => format!("resume {id}"),
    }
}

async fn read_line() -> Result<Option<String>> {
    tokio::task::spawn_blocking(|| {
        let mut line = String::new();
        match io::stdin().read_line(&mut line) {
            Ok(0) => Ok(None),
            Ok(_) => Ok(Some(line)),
            Err(e) => Err(anyhow::anyhow!("stdin: {e}")),
        }
    })
    .await?
}

/// Entry used by `main`: parse argv, run, map errors to exit codes.
pub async fn main_from_env() -> i32 {
    dotenv::dotenv().ok();

    let argv: Vec<String> = std::env::args().skip(1).collect();
    let cli = match super::parse_args(&argv) {
        Ok(None) => return 0,
        Ok(Some(cli)) => cli,
        Err(e) => {
            eprintln!("[agent-v1] error: {e}");
            print_usage();
            return 2;
        }
    };

    match run(cli).await {
        Ok(code) => code,
        Err(e) => {
            eprintln!("[agent-v1] error: {e:#}");
            1
        }
    }
}
