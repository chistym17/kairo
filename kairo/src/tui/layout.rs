//! Responsive layout helpers for wide / medium / narrow terminals.

use ratatui::layout::{Constraint, Direction, Layout, Rect};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WidthMode {
    /// ≥ 120 columns — conversation + full tabbed sidebar
    Wide,
    /// 80–119 columns — conversation + narrow sidebar
    Medium,
    /// < 80 columns — conversation only; sidebar becomes a strip / overlay later
    Narrow,
}

impl WidthMode {
    pub fn from_cols(cols: u16) -> Self {
        if cols >= 120 {
            Self::Wide
        } else if cols >= 80 {
            Self::Medium
        } else {
            Self::Narrow
        }
    }

    pub fn sidebar_pct(self) -> u16 {
        match self {
            Self::Wide => 28,
            Self::Medium => 32,
            Self::Narrow => 0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ShellLayout {
    pub header: Rect,
    pub conversation: Rect,
    pub sidebar: Option<Rect>,
    pub narrow_plan_strip: Option<Rect>,
    pub composer: Rect,
    pub status: Rect,
    pub width_mode: WidthMode,
}

pub fn compute(area: Rect, composer_height: u16) -> ShellLayout {
    let width_mode = WidthMode::from_cols(area.width);
    let composer_h = composer_height.clamp(3, 8);

    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),       // header
            Constraint::Min(8),          // body
            Constraint::Length(composer_h),
            Constraint::Length(1),       // status
        ])
        .split(area);

    let header = vertical[0];
    let body = vertical[1];
    let composer = vertical[2];
    let status = vertical[3];

    match width_mode {
        WidthMode::Wide | WidthMode::Medium => {
            let side = width_mode.sidebar_pct();
            let main = 100u16.saturating_sub(side);
            let cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(main), Constraint::Percentage(side)])
                .split(body);
            ShellLayout {
                header,
                conversation: cols[0],
                sidebar: Some(cols[1]),
                narrow_plan_strip: None,
                composer,
                status,
                width_mode,
            }
        }
        WidthMode::Narrow => {
            let rows = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(4), Constraint::Length(3)])
                .split(body);
            ShellLayout {
                header,
                conversation: rows[0],
                sidebar: None,
                narrow_plan_strip: Some(rows[1]),
                composer,
                status,
                width_mode,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn width_breakpoints() {
        assert_eq!(WidthMode::from_cols(140), WidthMode::Wide);
        assert_eq!(WidthMode::from_cols(120), WidthMode::Wide);
        assert_eq!(WidthMode::from_cols(119), WidthMode::Medium);
        assert_eq!(WidthMode::from_cols(80), WidthMode::Medium);
        assert_eq!(WidthMode::from_cols(79), WidthMode::Narrow);
    }

    #[test]
    fn narrow_hides_sidebar() {
        let area = Rect::new(0, 0, 60, 30);
        let layout = compute(area, 5);
        assert!(layout.sidebar.is_none());
        assert!(layout.narrow_plan_strip.is_some());
    }

    #[test]
    fn wide_shows_sidebar() {
        let area = Rect::new(0, 0, 140, 40);
        let layout = compute(area, 5);
        assert!(layout.sidebar.is_some());
        assert!(layout.narrow_plan_strip.is_none());
    }
}
