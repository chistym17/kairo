//! Rendering for the static responsive shell.

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

use super::app::{App, PlanStepStatus, SidebarTab, TimelineItem};
use super::layout::{self, WidthMode};

pub fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let shell = layout::compute(area, app.composer.height);

    render_header(frame, shell.header, app);
    render_conversation(frame, shell.conversation, app);

    if let Some(side) = shell.sidebar {
        render_sidebar(frame, side, app, shell.width_mode);
    }
    if let Some(strip) = shell.narrow_plan_strip {
        render_narrow_plan_strip(frame, strip, app);
    }

    render_composer(frame, shell.composer, app);
    render_status(frame, shell.status, app, shell.width_mode);
}

fn render_header(frame: &mut Frame, area: Rect, app: &App) {
    let left = format!(
        " {}   {}   {}   {}",
        app.header.brand,
        app.header.mode.as_str(),
        app.header.model,
        app.header.workspace,
    );
    let right = format!("{} ", app.header.edit_policy);

    let line = Line::from(vec![
        Span::styled(
            left,
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::raw(right),
    ]);

    let block = Block::default().borders(Borders::BOTTOM);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new(line).alignment(Alignment::Left),
        inner,
    );
}

fn render_conversation(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Conversation ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();
    for item in &app.conversation.items {
        match item {
            TimelineItem::User(text) => {
                lines.push(Line::from(Span::styled(
                    "You",
                    Style::default().add_modifier(Modifier::BOLD),
                )));
                push_wrapped(&mut lines, text);
                lines.push(Line::from(""));
            }
            TimelineItem::Assistant(text) => {
                lines.push(Line::from(Span::styled(
                    "Kairo",
                    Style::default().add_modifier(Modifier::BOLD),
                )));
                push_wrapped(&mut lines, text);
                lines.push(Line::from(""));
            }
            TimelineItem::Activity { title, lines: body } => {
                lines.push(Line::from(Span::styled(
                    title.clone(),
                    Style::default().add_modifier(Modifier::BOLD),
                )));
                for row in body {
                    lines.push(Line::from(row.clone()));
                }
                lines.push(Line::from(""));
            }
            TimelineItem::ResultCard {
                title,
                summary,
                changed,
                verification,
                footer,
            } => {
                lines.push(Line::from(format!("╭─ {title}")));
                lines.push(Line::from("│"));
                lines.push(Line::from(format!("│ {summary}")));
                lines.push(Line::from("│"));
                lines.push(Line::from("│ Changed"));
                for c in changed {
                    lines.push(Line::from(format!("│ {c}")));
                }
                lines.push(Line::from("│"));
                lines.push(Line::from("│ Verification"));
                for v in verification {
                    lines.push(Line::from(format!("│ {v}")));
                }
                lines.push(Line::from("│"));
                lines.push(Line::from(format!("│ {footer}")));
                lines.push(Line::from("╰─"));
                lines.push(Line::from(""));
            }
        }
    }

    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((app.conversation.scroll, 0)),
        inner,
    );
}

fn push_wrapped(lines: &mut Vec<Line>, text: &str) {
    for paragraph in text.split('\n') {
        lines.push(Line::from(paragraph.to_string()));
    }
}

fn render_sidebar(frame: &mut Frame, area: Rect, app: &App, mode: WidthMode) {
    let tabs = tab_line(app.sidebar.tab, mode);
    let block = Block::default().borders(Borders::ALL).title(tabs);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let body = match app.sidebar.tab {
        SidebarTab::Plan => render_plan_lines(app),
        SidebarTab::Changes => render_changes_lines(app),
        SidebarTab::Context => render_context_lines(app),
    };

    frame.render_widget(Paragraph::new(body).wrap(Wrap { trim: false }), inner);
}

fn tab_line(active: SidebarTab, mode: WidthMode) -> Line<'static> {
    let mut spans = Vec::new();
    for (i, tab) in SidebarTab::ALL.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(" "));
        }
        let label = if mode == WidthMode::Medium {
            // Shorter chrome on medium widths.
            match tab {
                SidebarTab::Plan => "Plan",
                SidebarTab::Changes => "Diff",
                SidebarTab::Context => "Info",
            }
        } else {
            tab.title()
        };
        if *tab == active {
            spans.push(Span::styled(
                format!("[{label}]"),
                Style::default().add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            ));
        } else {
            spans.push(Span::raw(format!(" {label} ")));
        }
    }
    Line::from(spans)
}

fn render_plan_lines(app: &App) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(format!(
        "PLAN  {}/{}",
        app.sidebar.plan_done, app.sidebar.plan_total
    ))];
    lines.push(Line::from(""));
    for step in &app.sidebar.plan_steps {
        let mark = match step.status {
            PlanStepStatus::Done => "✓",
            PlanStepStatus::Active => "●",
            PlanStepStatus::Todo => "○",
        };
        lines.push(Line::from(format!("{mark} {}", step.text)));
    }
    lines
}

fn render_changes_lines(app: &App) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from("CHANGES"), Line::from("")];
    if app.sidebar.changes.is_empty() {
        lines.push(Line::from("No file changes yet."));
    } else {
        for change in &app.sidebar.changes {
            lines.push(Line::from(format!("M {}", change.path)));
            lines.push(Line::from(format!("  {}", change.stats)));
            lines.push(Line::from(""));
        }
        lines.push(Line::from("Enter  View diff"));
    }
    lines
}

fn render_context_lines(app: &App) -> Vec<Line<'static>> {
    let s = &app.sidebar.session;
    vec![
        Line::from("SESSION"),
        Line::from(""),
        Line::from(format!("Mode       {}", s.mode)),
        Line::from(format!("Policy     {}", s.policy)),
        Line::from(format!("Model      {}", s.model)),
        Line::from(format!("Workspace  {}", s.workspace)),
        Line::from(format!("Session    {}", s.session_id)),
        Line::from(format!("Tokens     {}", s.tokens)),
        Line::from(format!("Elapsed    {}", s.elapsed)),
    ]
}

fn render_narrow_plan_strip(frame: &mut Frame, area: Rect, app: &App) {
    let active = app
        .sidebar
        .plan_steps
        .iter()
        .find(|s| s.status == PlanStepStatus::Active)
        .map(|s| s.text.as_str())
        .unwrap_or("…");
    let text = format!(
        " Plan {}/{} · {active} ",
        app.sidebar.plan_done, app.sidebar.plan_total
    );
    let block = Block::default().borders(Borders::ALL);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(text), inner);
}

fn render_composer(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::default().borders(Borders::ALL).title(" Input ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let (text, style) = if app.composer.text.is_empty() {
        (
            app.composer.placeholder.as_str(),
            Style::default().add_modifier(Modifier::DIM),
        )
    } else {
        (app.composer.text.as_str(), Style::default())
    };

    frame.render_widget(
        Paragraph::new(Span::styled(text.to_string(), style)).wrap(Wrap { trim: false }),
        inner,
    );

    // Place cursor at end of typed text (approx: first row, col = len).
    if !app.composer.text.is_empty() {
        let x = inner.x.saturating_add(app.composer.text.len() as u16);
        let x = x.min(inner.x.saturating_add(inner.width.saturating_sub(1)));
        frame.set_cursor_position((x, inner.y));
    } else {
        frame.set_cursor_position((inner.x, inner.y));
    }
}

fn render_status(frame: &mut Frame, area: Rect, app: &App, mode: WidthMode) {
    let mode_label = match mode {
        WidthMode::Wide => "wide",
        WidthMode::Medium => "medium",
        WidthMode::Narrow => "narrow",
    };
    let text = format!(" {}  ·  layout:{mode_label} ", app.status.hint);
    frame.render_widget(
        Paragraph::new(text).style(Style::default().add_modifier(Modifier::DIM)),
        area,
    );
}

#[allow(dead_code)]
fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup[1])[1]
}

#[allow(dead_code)]
fn render_clear(frame: &mut Frame, area: Rect) {
    frame.render_widget(Clear, area);
}
