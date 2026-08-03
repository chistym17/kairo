//! Keyboard handling for the static shell.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::app::{App, SidebarTab};

pub enum Action {
    Quit,
    Continue,
}

pub fn handle_key(key: KeyEvent, app: &mut App) -> Action {
    // Ignore key-release / repeat noise on backends that emit them.
    if key.kind != crossterm::event::KeyEventKind::Press {
        return Action::Continue;
    }

    match (key.modifiers, key.code) {
        (KeyModifiers::CONTROL, KeyCode::Char('c'))
        | (KeyModifiers::NONE, KeyCode::Char('q'))
        | (KeyModifiers::NONE, KeyCode::Esc) => Action::Quit,

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
            app.composer.text.push(c);
            Action::Continue
        }
        (KeyModifiers::SHIFT, KeyCode::Char(c)) => {
            app.composer.text.push(c);
            Action::Continue
        }
        (KeyModifiers::NONE, KeyCode::Backspace) => {
            app.composer.text.pop();
            Action::Continue
        }
        (KeyModifiers::NONE, KeyCode::Enter) => {
            // Part 1: local echo only — no agent wiring yet.
            let line = app.composer.text.trim().to_string();
            if !line.is_empty() {
                app.conversation
                    .items
                    .push(super::app::TimelineItem::User(line));
                app.composer.text.clear();
            }
            Action::Continue
        }
        _ => Action::Continue,
    }
}
