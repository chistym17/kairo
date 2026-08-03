//! Full-screen Ratatui shell for interactive Kairo sessions.
//!
//! Part 1 ships a static responsive layout with demo content so we can
//! validate the chrome before wiring the live `AgentEvent` stream.

mod app;
mod demo;
mod draw;
mod input;
mod layout;

use std::io::{self, stdout};
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use app::App;
use input::Action;

/// Run the static TUI shell until the user quits.
pub fn run() -> Result<()> {
    let mut terminal = setup()?;
    let mut app = App::demo();

    let result = run_loop(&mut terminal, &mut app);

    restore()?;
    result
}

fn run_loop(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, app: &mut App) -> Result<()> {
    loop {
        terminal.draw(|frame| draw::render(frame, app))?;

        if !event::poll(Duration::from_millis(100)).context("poll terminal events")? {
            continue;
        }

        match event::read().context("read terminal event")? {
            Event::Key(key) => match input::handle_key(key, app) {
                Action::Quit => return Ok(()),
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
