//! Quiet 3-tone palette for the TUI — accent, muted, default. No neon.

use ratatui::style::{Color, Modifier, Style};

/// Brand / active / speaker labels.
pub fn accent() -> Style {
    Style::default().fg(Color::Cyan)
}

pub fn accent_bold() -> Style {
    accent().add_modifier(Modifier::BOLD)
}

/// Borders, placeholders, hints, inactive chrome.
pub fn muted() -> Style {
    Style::default().fg(Color::DarkGray)
}

pub fn muted_dim() -> Style {
    muted().add_modifier(Modifier::DIM)
}

/// Body copy.
pub fn body() -> Style {
    Style::default()
}

pub fn border() -> Style {
    muted()
}

pub fn border_focus() -> Style {
    accent()
}

/// Active sidebar tab.
pub fn tab_active() -> Style {
    accent_bold().add_modifier(Modifier::UNDERLINED)
}

pub fn tab_inactive() -> Style {
    muted()
}

/// Inset content so it does not sit on the border line.
pub fn pad(area: ratatui::layout::Rect, horizontal: u16, vertical: u16) -> ratatui::layout::Rect {
    let h = horizontal.min(area.width / 2);
    let v = vertical.min(area.height / 2);
    ratatui::layout::Rect {
        x: area.x.saturating_add(h),
        y: area.y.saturating_add(v),
        width: area.width.saturating_sub(h.saturating_mul(2)),
        height: area.height.saturating_sub(v.saturating_mul(2)),
    }
}
