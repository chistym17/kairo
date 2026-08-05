//! Full-screen Ratatui shell for interactive Kairo sessions.
//!
//! Screens: Welcome → Session (live agent) / Sessions browser.
//! Session panels wire the live [`crate::api::AgentEvent`] stream.

mod app;
mod composer;
mod demo;
mod draw;
mod feed;
mod input;
mod layout;
mod overlay;
mod present;
mod screens;
mod theme;

use std::io::{self, stdout, IsTerminal};
use std::path::PathBuf;
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
use crate::session::SessionManager;
use crate::user_config;

use app::{App, Screen, TimelineItem};
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
    pub workspace: Option<PathBuf>,
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

    // TUI owns the alternate screen — never println to stdout (even with --verbose).
    // Diagnostics still land in the run JSONL log.
    logging::configure_cli_quiet(opts.verbose);
    logging::set_quiet(true);
    user_config::ensure_configured(true).context("provider setup")?;

    let workspace = detect_workspace(opts.workspace).context("failed to detect workspace")?;
    let edit_policy = opts.edit_policy.unwrap_or(EditPolicy::Confirm);
    let policy_label = match edit_policy {
        EditPolicy::Confirm => "Confirm edits",
        EditPolicy::Auto => "Auto edits",
        EditPolicy::ReadOnly => "Read-only",
    };

    let (provider, provider_id, model) = resolve_provider_model();
    let mut app = App::live_session(
        &model,
        &display_path(&workspace),
        "Agent",
        policy_label,
        "—",
        workspace.clone(),
    )
    .with_welcome(
        &provider,
        &provider_id,
        &model,
        &display_path(&workspace),
        policy_label,
    );

    let mut terminal = setup()?;
    let result = run_shell(&mut terminal, &mut app, &workspace, edit_policy).await;
    restore()?;
    result
}

fn resolve_provider_model() -> (String, String, String) {
    match user_config::load() {
        Ok(Some(cfg)) => (
            user_config::provider_label(&cfg.provider).to_string(),
            cfg.provider.to_ascii_lowercase(),
            cfg.model,
        ),
        _ => {
            let provider_id = std::env::var("LLM_PROVIDER")
                .unwrap_or_else(|_| "deepseek".into())
                .to_ascii_lowercase();
            let model = std::env::var("MODEL_ID")
                .unwrap_or_else(|_| user_config::default_model(&provider_id).to_string());
            (
                user_config::provider_label(&provider_id).to_string(),
                provider_id,
                model,
            )
        }
    }
}

async fn run_shell(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    workspace: &PathBuf,
    edit_policy: EditPolicy,
) -> Result<()> {
    loop {
        match app.screen {
            Screen::Welcome | Screen::Sessions => {
                match run_menu_loop(terminal, app, workspace)? {
                    MenuNext::Quit => return Ok(()),
                    MenuNext::StartNew => {
                        open_and_run_session(
                            terminal,
                            app,
                            workspace,
                            edit_policy,
                            SessionSource::Persistent,
                            None,
                        )
                        .await?;
                    }
                    MenuNext::Resume(id) => {
                        open_and_run_session(
                            terminal,
                            app,
                            workspace,
                            edit_policy,
                            SessionSource::Resume(id.clone()),
                            Some(id),
                        )
                        .await?;
                    }
                }
            }
            Screen::Session => {
                // Should not idle on Session without an agent; return to welcome.
                app.show_welcome();
            }
        }
    }
}

enum MenuNext {
    Quit,
    StartNew,
    Resume(String),
}

fn run_menu_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    workspace: &PathBuf,
) -> Result<MenuNext> {
    loop {
        terminal.draw(|frame| draw::render(frame, app))?;

        if !event::poll(Duration::from_millis(120)).context("poll terminal events")? {
            continue;
        }

        match event::read().context("read terminal event")? {
            Event::Key(key) => match input::handle_key(key, app) {
                Action::Quit => return Ok(MenuNext::Quit),
                Action::StartNewSession => return Ok(MenuNext::StartNew),
                Action::ResumeSelected => {
                    if let Some(id) = app.sessions.selected_id().map(str::to_string) {
                        return Ok(MenuNext::Resume(id));
                    }
                }
                Action::OpenSessions => {
                    let entries = SessionManager::with_default_dir()
                        .list()
                        .unwrap_or_default();
                    app.workspace_path = workspace.clone();
                    app.show_sessions(entries);
                }
                Action::Continue => {}
                _ => {}
            },
            Event::Resize(width, height) => {
                app.terminal_size = (width, height);
            }
            _ => {}
        }
    }
}

async fn open_and_run_session(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    workspace: &PathBuf,
    edit_policy: EditPolicy,
    source: SessionSource,
    resume_id: Option<String>,
) -> Result<()> {
    if let Some(id) = &resume_id {
        if let Ok(persisted) = SessionManager::with_default_dir().load(id) {
            app.hydrate_from_persisted(&persisted);
        }
    } else {
        app.conversation.items = vec![TimelineItem::Assistant(
            "Session ready. Type a task and press Enter.".into(),
        )];
        app.sidebar.plan_steps.clear();
        app.sidebar.plan_done = 0;
        app.sidebar.plan_total = 0;
        app.sidebar.changes.clear();
        app.run = Default::default();
    }

    let config = AgentConfig::builder()
        .workspace(workspace)
        .mode(AgentMode::Agent)
        .edit_policy(edit_policy)
        .session(source)
        .build()
        .context("invalid agent config")?;

    let model = config.model().to_string();
    let session = AgentSession::open(config).context("failed to open session")?;
    let mut events = session.subscribe().context("event stream")?;

    app.header.model = model.clone();
    app.sidebar.session.model = model;
    let sid = session.session_id();
    app.sidebar.session.session_id = if sid.len() > 8 {
        format!("{}…", &sid[..8])
    } else {
        sid.to_string()
    };
    app.workspace_path = workspace.clone();
    app.enter_session_screen();

    let leave = run_live_loop(terminal, app, &session, &mut events).await;
    let _ = session.shutdown();
    leave?;
    app.show_welcome();
    Ok(())
}

fn run_demo_shell() -> Result<()> {
    let mut terminal = setup()?;
    let mut app = App::demo();
    app.screen = Screen::Session;
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
                Action::Quit | Action::LeaveSession | Action::CancelRun => return Ok(()),
                Action::Send(line) => {
                    app.conversation.items.push(TimelineItem::User(line));
                }
                Action::Continue
                | Action::ApproveOnce
                | Action::ApproveForTask
                | Action::Reject
                | Action::StartNewSession
                | Action::ResumeSelected
                | Action::OpenSessions
                | Action::CycleMode => {
                    feed::refresh_status_hint(app);
                }
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
                Action::Quit | Action::LeaveSession => return Ok(()),
                Action::CancelRun => {
                    session.request_stop();
                    app.run.set_current("Cancel requested…".into());
                    refresh_status_hint(app);
                }
                Action::CycleMode => {
                    app.cycle_mode();
                    if let Err(e) = session.set_mode(app.api_mode()) {
                        app.conversation.items.push(TimelineItem::Assistant(format!(
                            "Could not switch mode: {e:#}"
                        )));
                    } else {
                        let label = app.header.mode.as_str();
                        app.conversation.items.push(TimelineItem::Assistant(format!(
                            "Switched to {label} mode ({}).",
                            app.header.edit_policy
                        )));
                    }
                    refresh_status_hint(app);
                }
                Action::Send(line) => {
                    app.conversation.follow_tail = true;
                    app.conversation
                        .items
                        .push(TimelineItem::User(line.clone()));
                    app.run.begin();
                    app.overlay = None;
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
                        app.clear_approval_overlay();
                        app.run.clear_current();
                        refresh_status_hint(app);
                    }
                }
                Action::ApproveForTask => {
                    if let Some(req) = app.run.pending_approval.take() {
                        let _ = session.approve_for_task(&req.id).await;
                        app.clear_approval_overlay();
                        app.run.clear_current();
                        refresh_status_hint(app);
                    }
                }
                Action::Reject => {
                    if let Some(req) = app.run.pending_approval.take() {
                        let _ = session.reject(&req.id).await;
                        app.clear_approval_overlay();
                        app.run.clear_current();
                        refresh_status_hint(app);
                    }
                }
                Action::Continue
                | Action::StartNewSession
                | Action::ResumeSelected
                | Action::OpenSessions => {}
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
