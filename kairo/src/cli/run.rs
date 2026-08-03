use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::api::{AgentConfig, AgentEvent, AgentMode, AgentSession, EditPolicy, RunResult, SessionSource};
use crate::logging;
use crate::user_config;

use super::args::{CliArgs, Command, OutputFormat, SessionArg};
use super::output::{exit_code, EventSink};
use super::print_usage;
use super::workspace::{current_branch, detect_workspace, display_path};

pub async fn run(cli: CliArgs) -> Result<i32> {
    if matches!(cli.command, Command::Health) {
        return Ok(super::health::run());
    }

    if matches!(cli.command, Command::Config) {
        user_config::run_config_menu().context("config")?;
        return Ok(0);
    }

    if matches!(cli.command, Command::Tui) {
        crate::tui::run().context("tui")?;
        return Ok(0);
    }

    // Product CLI: quiet harness debug by default; --verbose / KAIRO_VERBOSE restores it.
    logging::configure_cli_quiet(cli.verbose);

    let interactive_setup = io::stdin().is_terminal()
        && !matches!(cli.output, OutputFormat::Jsonl);
    user_config::ensure_configured(interactive_setup)
        .context("provider setup")?;

    let workspace = resolve_workspace(cli.workspace.clone())?;
    print_workspace_banner(&workspace, matches!(cli.output, OutputFormat::Jsonl));

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

async fn run_oneshot(
    session: &AgentSession,
    sink: &mut EventSink,
    task: String,
) -> Result<i32> {
    let mut events = session.subscribe().context("event stream")?;
    sink.begin_turn();
    match run_turn(session, &mut events, sink, task).await {
        Ok(result) => Ok(exit_code(&result)),
        Err(e) => {
            sink.turn_failed(&e);
            Ok(1)
        }
    }
}

async fn run_interactive(session: &AgentSession, sink: &mut EventSink) -> Result<i32> {
    let mut events = session.subscribe().context("event stream")?;
    sink.info("Type a task. /help for commands, /exit to quit.");

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
            sink.info("Bye.");
            break;
        }
        if line == "/help" {
            eprintln!("commands: /exit  /help  /config");
            eprintln!("type any other line to send it as a task");
            continue;
        }
        if line == "/config" {
            eprintln!("Run `kairo config` in another terminal, or exit and reconfigure.");
            continue;
        }

        sink.begin_turn();
        match run_turn(session, &mut events, sink, line).await {
            Ok(result) => {
                last_code = exit_code(&result);
            }
            Err(e) => {
                // Keep the session open so the user can retry.
                sink.turn_failed(&e);
                last_code = 1;
            }
        }
    }
    Ok(last_code)
}

async fn run_turn(
    session: &AgentSession,
    events: &mut crate::api::EventStream,
    sink: &mut EventSink,
    task: String,
) -> Result<RunResult> {
    session.send(task).await.context("failed to start turn")?;
    drain_turn(session, events, sink).await
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
            let id = request.id.clone();
            if let Err(e) = sink.handle_approval(session, request).await {
                // Unblock the worker so this turn can finish; keep the REPL alive.
                let _ = session.reject(id).await;
                sink.turn_failed(&e);
            }
        }

        if let AgentEvent::Finished { result } = event {
            return Ok(result);
        }
    }
}

fn resolve_workspace(cli_workspace: Option<PathBuf>) -> Result<PathBuf> {
    detect_workspace(cli_workspace).context("failed to detect workspace")
}

fn print_workspace_banner(workspace: &std::path::Path, jsonl: bool) {
    let branch = current_branch(workspace).unwrap_or_else(|| "(not a git repo)".to_string());
    let line_ws = format!("Workspace: {}", display_path(workspace));
    let line_br = format!("Branch: {branch}");
    if jsonl {
        eprintln!("[kairo] {line_ws}");
        eprintln!("[kairo] {line_br}");
    } else {
        println!("{line_ws}");
        println!("{line_br}");
    }
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
            eprintln!("[kairo] error: {e}");
            print_usage();
            return 2;
        }
    };

    match run(cli).await {
        Ok(code) => code,
        Err(e) => {
            eprintln!("[kairo] error: {e:#}");
            1
        }
    }
}
