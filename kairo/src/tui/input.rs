//! Keyboard handling for the live TUI shell.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::app::{App, SidebarTab};

pub enum Action {
    Quit,
    Continue,
    /// Send composer text as a task (cleared by the caller after queueing).
    Send(String),
    ApproveOnce,
    ApproveForTask,
    Reject,
    /// Cooperative cancel of the in-flight run.
    CancelRun,
}

pub fn handle_key(key: KeyEvent, app: &mut App) -> Action {
    if key.kind != crossterm::event::KeyEventKind::Press {
        return Action::Continue;
    }

    // Approval decisions take priority while a gate is open.
    if app.run.pending_approval.is_some() {
        match (key.modifiers, key.code) {
            (KeyModifiers::NONE, KeyCode::Char('a')) => return Action::ApproveOnce,
            (KeyModifiers::NONE, KeyCode::Char('t')) => return Action::ApproveForTask,
            (KeyModifiers::NONE, KeyCode::Char('r')) => return Action::Reject,
            (KeyModifiers::CONTROL, KeyCode::Char('c'))
            | (KeyModifiers::NONE, KeyCode::Char('q')) => return Action::Quit,
            (KeyModifiers::NONE, KeyCode::Esc) => return Action::Reject,
            _ => {}
        }
    }

    match (key.modifiers, key.code) {
        (KeyModifiers::CONTROL, KeyCode::Char('c')) | (KeyModifiers::NONE, KeyCode::Char('q')) => {
            Action::Quit
        }
        (KeyModifiers::NONE, KeyCode::Esc) => {
            if app.run.busy {
                Action::CancelRun
            } else {
                Action::Quit
            }
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
        (KeyModifiers::NONE, KeyCode::Char(c)) if !c.is_control() => {
            if !app.run.busy {
                app.composer.text.push(c);
            }
            Action::Continue
        }
        (KeyModifiers::SHIFT, KeyCode::Char(c)) => {
            if !app.run.busy {
                app.composer.text.push(c);
            }
            Action::Continue
        }
        (KeyModifiers::NONE, KeyCode::Backspace) => {
            if !app.run.busy {
                app.composer.text.pop();
            }
            Action::Continue
        }
        (KeyModifiers::NONE, KeyCode::Enter) => {
            if app.run.busy {
                return Action::Continue;
            }
            let line = app.composer.text.trim().to_string();
            if line.is_empty() {
                return Action::Continue;
            }
            app.composer.text.clear();
            Action::Send(line)
        }
        _ => Action::Continue,
    }
}
