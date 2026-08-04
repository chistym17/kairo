//! Keyboard handling for the live TUI shell.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::app::{App, Screen, SidebarTab};
use super::overlay::Overlay;

pub enum Action {
    Quit,
    Continue,
    /// Send composer text as a task.
    Send(String),
    ApproveOnce,
    ApproveForTask,
    Reject,
    /// Cooperative cancel of the in-flight run.
    CancelRun,
    /// Leave the active session and return to Welcome.
    LeaveSession,
    /// Open a new agent session from Welcome / Sessions.
    StartNewSession,
    /// Resume the selected session from the browser.
    ResumeSelected,
    /// Open the sessions browser.
    OpenSessions,
}

pub fn handle_key(key: KeyEvent, app: &mut App) -> Action {
    if key.kind != crossterm::event::KeyEventKind::Press {
        return Action::Continue;
    }

    match app.screen {
        Screen::Welcome => return handle_welcome_key(key, app),
        Screen::Sessions => return handle_sessions_key(key, app),
        Screen::Session => {}
    }

    // Modal overlays capture keys first.
    if app.overlay.is_some() {
        return handle_overlay_key(key, app);
    }

    match (key.modifiers, key.code) {
        (KeyModifiers::CONTROL, KeyCode::Char('c')) => Action::Quit,
        (KeyModifiers::NONE, KeyCode::Char('q')) if app.composer.is_empty() && !app.run.busy => {
            Action::LeaveSession
        }

        (KeyModifiers::NONE, KeyCode::Char('?')) => {
            app.overlay = Some(Overlay::Help);
            Action::Continue
        }

        (KeyModifiers::NONE, KeyCode::Esc) => {
            if app.run.busy {
                Action::CancelRun
            } else if !app.composer.is_empty() {
                app.composer.clear();
                Action::Continue
            } else {
                Action::LeaveSession
            }
        }

        (KeyModifiers::CONTROL, KeyCode::Char('l')) => {
            app.clear_conversation();
            Action::Continue
        }

        (KeyModifiers::CONTROL, KeyCode::Char('p')) => {
            app.set_sidebar_tab(SidebarTab::Plan);
            Action::Continue
        }
        (KeyModifiers::CONTROL, KeyCode::Char('d')) => {
            app.set_sidebar_tab(SidebarTab::Changes);
            Action::Continue
        }
        (KeyModifiers::CONTROL, KeyCode::Char('i')) => {
            app.set_sidebar_tab(SidebarTab::Context);
            Action::Continue
        }
        (KeyModifiers::NONE, KeyCode::Tab) => {
            app.cycle_mode();
            Action::Continue
        }

        _ if app.run.busy => Action::Continue,

        (KeyModifiers::NONE, KeyCode::Up)
            if app.sidebar.tab == SidebarTab::Changes && app.composer.is_empty() =>
        {
            app.select_change_delta(-1);
            Action::Continue
        }
        (KeyModifiers::NONE, KeyCode::Down)
            if app.sidebar.tab == SidebarTab::Changes && app.composer.is_empty() =>
        {
            app.select_change_delta(1);
            Action::Continue
        }
        (KeyModifiers::NONE, KeyCode::Enter)
            if app.sidebar.tab == SidebarTab::Changes
                && app.composer.is_empty()
                && !app.sidebar.changes.is_empty() =>
        {
            app.open_diff_for_selected_change();
            Action::Continue
        }

        (KeyModifiers::SHIFT, KeyCode::Enter) | (KeyModifiers::ALT, KeyCode::Enter) => {
            app.composer.insert_newline();
            Action::Continue
        }
        (KeyModifiers::CONTROL, KeyCode::Char('j')) => {
            app.composer.insert_newline();
            Action::Continue
        }
        (KeyModifiers::NONE, KeyCode::Enter) => match app.composer.take_for_send() {
            Some(line) => Action::Send(line),
            None => Action::Continue,
        },

        (KeyModifiers::NONE, KeyCode::Backspace) | (KeyModifiers::NONE, KeyCode::Delete) => {
            if key.code == KeyCode::Delete {
                app.composer.delete_forward();
            } else {
                app.composer.backspace();
            }
            Action::Continue
        }
        (KeyModifiers::NONE, KeyCode::Left) => {
            app.composer.move_left();
            Action::Continue
        }
        (KeyModifiers::NONE, KeyCode::Right) => {
            app.composer.move_right();
            Action::Continue
        }
        (KeyModifiers::NONE, KeyCode::Up) => {
            app.composer.up();
            Action::Continue
        }
        (KeyModifiers::NONE, KeyCode::Down) => {
            app.composer.down();
            Action::Continue
        }
        (KeyModifiers::NONE, KeyCode::Home) => {
            app.composer.move_home();
            Action::Continue
        }
        (KeyModifiers::NONE, KeyCode::End) => {
            app.composer.move_end();
            Action::Continue
        }

        (KeyModifiers::NONE, KeyCode::Char(c)) if !c.is_control() => {
            app.composer.insert_char(c);
            Action::Continue
        }
        (KeyModifiers::SHIFT, KeyCode::Char(c)) => {
            app.composer.insert_char(c);
            Action::Continue
        }

        _ => Action::Continue,
    }
}

fn handle_welcome_key(key: KeyEvent, app: &mut App) -> Action {
    if matches!(app.overlay, Some(Overlay::ModelPicker(_))) {
        return handle_model_picker_key(key, app);
    }

    match (key.modifiers, key.code) {
        (KeyModifiers::CONTROL, KeyCode::Char('c'))
        | (KeyModifiers::NONE, KeyCode::Char('q'))
        | (KeyModifiers::NONE, KeyCode::Esc) => Action::Quit,
        (KeyModifiers::NONE, KeyCode::Enter) => Action::StartNewSession,
        (KeyModifiers::NONE, KeyCode::Char('s')) => Action::OpenSessions,
        (KeyModifiers::NONE, KeyCode::Char('m')) => {
            app.open_model_picker();
            Action::Continue
        }
        _ => Action::Continue,
    }
}

fn handle_model_picker_key(key: KeyEvent, app: &mut App) -> Action {
    match (key.modifiers, key.code) {
        (KeyModifiers::NONE, KeyCode::Esc) => {
            app.overlay = None;
            app.status.hint =
                "Enter start · s sessions · m change model · q quit".into();
            Action::Continue
        }
        (KeyModifiers::NONE, KeyCode::Up) | (KeyModifiers::NONE, KeyCode::Char('k')) => {
            if let Some(Overlay::ModelPicker(p)) = app.overlay.as_mut() {
                p.move_sel(-1);
            }
            Action::Continue
        }
        (KeyModifiers::NONE, KeyCode::Down) | (KeyModifiers::NONE, KeyCode::Char('j')) => {
            if let Some(Overlay::ModelPicker(p)) = app.overlay.as_mut() {
                p.move_sel(1);
            }
            Action::Continue
        }
        (KeyModifiers::NONE, KeyCode::Enter) => match app.apply_selected_model() {
            Ok(()) => Action::Continue,
            Err(e) => {
                app.welcome.message = Some(format!("Could not save model: {e}"));
                app.overlay = None;
                Action::Continue
            }
        },
        (KeyModifiers::CONTROL, KeyCode::Char('c')) | (KeyModifiers::NONE, KeyCode::Char('q')) => {
            Action::Quit
        }
        _ => Action::Continue,
    }
}

fn handle_sessions_key(key: KeyEvent, app: &mut App) -> Action {
    match (key.modifiers, key.code) {
        (KeyModifiers::CONTROL, KeyCode::Char('c')) | (KeyModifiers::NONE, KeyCode::Char('q')) => {
            Action::Quit
        }
        (KeyModifiers::NONE, KeyCode::Esc) => {
            app.show_welcome();
            Action::Continue
        }
        (KeyModifiers::NONE, KeyCode::Char('n')) => Action::StartNewSession,
        (KeyModifiers::NONE, KeyCode::Enter) => {
            if app.sessions.entries.is_empty() {
                Action::StartNewSession
            } else {
                Action::ResumeSelected
            }
        }
        (KeyModifiers::NONE, KeyCode::Up) | (KeyModifiers::NONE, KeyCode::Char('k')) => {
            app.sessions.move_sel(-1);
            Action::Continue
        }
        (KeyModifiers::NONE, KeyCode::Down) | (KeyModifiers::NONE, KeyCode::Char('j')) => {
            app.sessions.move_sel(1);
            Action::Continue
        }
        _ => Action::Continue,
    }
}

fn handle_overlay_key(key: KeyEvent, app: &mut App) -> Action {
    let kind = match app.overlay.as_ref() {
        Some(Overlay::Approval(_)) => "approval",
        Some(Overlay::Diff(_)) => "diff",
        Some(Overlay::Help) => "help",
        Some(Overlay::ModelPicker(_)) => "model",
        None => return Action::Continue,
    };

    match kind {
        "model" => handle_model_picker_key(key, app),
        "approval" => match (key.modifiers, key.code) {
            (KeyModifiers::NONE, KeyCode::Char('a')) => Action::ApproveOnce,
            (KeyModifiers::NONE, KeyCode::Char('t')) => Action::ApproveForTask,
            (KeyModifiers::NONE, KeyCode::Char('r')) | (KeyModifiers::NONE, KeyCode::Esc) => {
                Action::Reject
            }
            (KeyModifiers::NONE, KeyCode::Char('d')) => {
                app.open_diff_for_approval();
                Action::Continue
            }
            (KeyModifiers::CONTROL, KeyCode::Char('c'))
            | (KeyModifiers::NONE, KeyCode::Char('q')) => Action::Quit,
            _ => Action::Continue,
        },
        "diff" => match (key.modifiers, key.code) {
            (KeyModifiers::NONE, KeyCode::Esc) => {
                app.close_overlay();
                Action::Continue
            }
            (KeyModifiers::NONE, KeyCode::Up) | (KeyModifiers::NONE, KeyCode::Char('k')) => {
                if let Some(Overlay::Diff(view)) = app.overlay.as_mut() {
                    view.scroll_by(-1, 20);
                }
                Action::Continue
            }
            (KeyModifiers::NONE, KeyCode::Down) | (KeyModifiers::NONE, KeyCode::Char('j')) => {
                if let Some(Overlay::Diff(view)) = app.overlay.as_mut() {
                    view.scroll_by(1, 20);
                }
                Action::Continue
            }
            (KeyModifiers::NONE, KeyCode::PageUp) => {
                if let Some(Overlay::Diff(view)) = app.overlay.as_mut() {
                    view.scroll_by(-10, 20);
                }
                Action::Continue
            }
            (KeyModifiers::NONE, KeyCode::PageDown) => {
                if let Some(Overlay::Diff(view)) = app.overlay.as_mut() {
                    view.scroll_by(10, 20);
                }
                Action::Continue
            }
            (KeyModifiers::CONTROL, KeyCode::Char('c')) => Action::Quit,
            _ => Action::Continue,
        },
        "help" => match (key.modifiers, key.code) {
            (KeyModifiers::NONE, KeyCode::Esc)
            | (KeyModifiers::NONE, KeyCode::Char('?'))
            | (KeyModifiers::NONE, KeyCode::Char('q')) => {
                app.overlay = None;
                Action::Continue
            }
            (KeyModifiers::CONTROL, KeyCode::Char('c')) => Action::Quit,
            _ => Action::Continue,
        },
        _ => Action::Continue,
    }
}
