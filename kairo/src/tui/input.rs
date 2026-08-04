//! Keyboard handling for the live TUI shell.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::app::{App, SidebarTab};

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
        (KeyModifiers::CONTROL, KeyCode::Char('c')) => Action::Quit,
        (KeyModifiers::NONE, KeyCode::Char('q')) if app.composer.is_empty() && !app.run.busy => {
            Action::Quit
        }

        (KeyModifiers::NONE, KeyCode::Esc) => {
            if app.run.busy {
                Action::CancelRun
            } else if !app.composer.is_empty() {
                app.composer.clear();
                Action::Continue
            } else {
                Action::Quit
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

        // Editing is locked while the agent runs.
        _ if app.run.busy => Action::Continue,

        (KeyModifiers::SHIFT, KeyCode::Enter) | (KeyModifiers::ALT, KeyCode::Enter) => {
            app.composer.insert_newline();
            Action::Continue
        }
        // Some terminals send Ctrl+J for newline.
        (KeyModifiers::CONTROL, KeyCode::Char('j')) => {
            app.composer.insert_newline();
            Action::Continue
        }
        (KeyModifiers::NONE, KeyCode::Enter) => match app.composer.take_for_send() {
            Some(line) => Action::Send(line),
            None => Action::Continue,
        },

        (KeyModifiers::NONE, KeyCode::Backspace)
        | (KeyModifiers::NONE, KeyCode::Delete) => {
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
            // Bare `q` quits only when empty (handled above); otherwise type it.
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
