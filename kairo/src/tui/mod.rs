//! Full-screen Ratatui shell for interactive Kairo sessions.
//!
//! Part 2 wires the live [`crate::api::AgentEvent`] stream into conversation,
//! Plan, Changes, and Context panels.

mod app;
mod demo;
mod draw;
mod feed;
mod input;
mod layout;

use std::io::{self, stdout, IsTerminal};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::api::{AgentConfig, AgentMode, AgentSession, EditPolicy, EventStream, SessionSource};
use crate::cli::{detect_workspace, display_path};
use crate::logging;
use crate::user_config;

use app::{App, TimelineItem};
use feed::{apply_event, refresh_status_hint, tick_spinner};
use input::Action;

/// Run the live TUI against a real agent session.
pub async fn run() -> Result<()> {
    run_with_options(TuiOptions::default()).await
}

#[derive(Debug, Clone, Default)]
pub struct TuiOptions {
    /// When true, skip the agent and show the static demo shell.
    pub demo: bool,
    pub workspace: Option<std::path::PathBuf>,
    pub edit_policy: Option<EditPolicy>,
    pub verbose: bool,
}

pub async fn run_with_options(opts: TuiOptions) -> Result<()> {
    if opts.demo {
        return run_demo_shell();
    }

    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        bail!("kairo tui requires an interactive terminal");
    }

    logging::configure_cli_quiet(opts.verbose);
    user_config::ensure_configured(true).context("provider setup")?;

    let workspace = detect_workspace(opts.workspace).context("failed to detect workspace")?;
    let edit_policy = opts.edit_policy.unwrap_or(EditPolicy::Confirm);

    let config = AgentConfig::builder()
        .workspace(&workspace)
        .mode(AgentMode::Agent)
        .edit_policy(edit_policy)
        .session(SessionSource::Persistent)
        .build()
        .context("invalid agent config")?;

    let model = config.model().to_string();
    let policy_label = match edit_policy {
        EditPolicy::Confirm => "Confirm edits",
        EditPolicy::Auto => "Auto edits",
        EditPolicy::ReadOnly => "Read-only",
    };

    let session = AgentSession::open(config).context("failed to open session")?;
    let mut events = session.subscribe().context("event stream")?;

    let mut app = App::live_session(
        &model,
        &display_path(&workspace),
        "Agent",
        policy_label,
        session.session_id(),
    );

    let mut terminal = setup()?;
    let result = run_live_loop(&mut terminal, &mut app, &session, &mut events).await;
    restore()?;
    let _ = session.shutdown();
    result
}

fn run_demo_shell() -> Result<()> {
    let mut terminal = setup()?;
    let mut app = App::demo();
    let result = run_demo_loop(&mut terminal, &mut app);
    restore()?;
    result
}

fn run_demo_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> Result<()> {
    loop {
        tick_spinner(app);
        terminal.draw(|frame| draw::render(frame, app))?;

        if !event::poll(Duration::from_millis(100)).context("poll terminal events")? {
            continue;
        }

        match event::read().context("read terminal event")? {
            Event::Key(key) => match input::handle_key(key, app) {
                Action::Quit | Action::CancelRun => return Ok(()),
                Action::Send(line) => {
                    app.conversation.items.push(TimelineItem::User(line));
                }
                Action::Continue
                | Action::ApproveOnce
                | Action::ApproveForTask
                | Action::Reject => {}
            },
            Event::Resize(width, height) => {
                app.terminal_size = (width, height);
            }
            _ => {}
        }
    }
}

async fn run_live_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    session: &AgentSession,
    events: &mut EventStream,
) -> Result<()> {
    loop {
        // Drain every buffered agent event before drawing.
        while let Some(ev) = events.try_recv() {
            apply_event(app, &ev);
        }
        tick_spinner(app);
        if app.run.busy {
            if let Some(started) = app.run.started_at {
                app.sidebar.session.elapsed =
                    feed::format_duration_ms(started.elapsed().as_millis() as u64);
            }
        }

        terminal.draw(|frame| draw::render(frame, app))?;

        if !event::poll(Duration::from_millis(80)).context("poll terminal events")? {
            continue;
        }

        match event::read().context("read terminal event")? {
            Event::Key(key) => match input::handle_key(key, app) {
                Action::Quit => return Ok(()),
                Action::CancelRun => {
                    session.request_stop();
                    app.run.set_current("Cancel requested…".into());
                    refresh_status_hint(app);
                }
                Action::Send(line) => {
                    app.conversation
                        .items
                        .push(TimelineItem::User(line.clone()));
                    app.run.begin();
                    refresh_status_hint(app);
                    if let Err(e) = session.send(line).await {
                        app.run.busy = false;
                        app.run.clear_current();
                        app.conversation
                            .items
                            .push(TimelineItem::Assistant(format!("Failed to start: {e:#}")));
                        refresh_status_hint(app);
                    }
                }
                Action::ApproveOnce => {
                    if let Some(req) = app.run.pending_approval.take() {
                        let _ = session.approve(&req.id).await;
                        app.run.clear_current();
                        refresh_status_hint(app);
                    }
                }
                Action::ApproveForTask => {
                    if let Some(req) = app.run.pending_approval.take() {
                        let _ = session.approve_for_task(&req.id).await;
                        app.run.clear_current();
                        refresh_status_hint(app);
                    }
                }
                Action::Reject => {
                    if let Some(req) = app.run.pending_approval.take() {
                        let _ = session.reject(&req.id).await;
                        app.run.clear_current();
                        refresh_status_hint(app);
                    }
                }
                Action::Continue => {}
            },
            Event::Resize(width, height) => {
                app.terminal_size = (width, height);
            }
            _ => {}
        }
    }
}

fn setup() -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode().context("enable raw mode")?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen, EnableMouseCapture).context("enter alternate screen")?;
    let backend = CrosstermBackend::new(out);
    Terminal::new(backend).context("create terminal")
}

fn restore() -> Result<()> {
    disable_raw_mode().context("disable raw mode")?;
    execute!(stdout(), LeaveAlternateScreen, DisableMouseCapture)
        .context("leave alternate screen")?;
    Ok(())
}
