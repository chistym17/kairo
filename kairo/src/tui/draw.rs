//! Rendering for the responsive TUI shell.

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

use super::app::{App, PlanStepStatus, SidebarTab, TimelineItem};
use super::feed::{self, spinner_glyph};
use super::layout::{self, WidthMode};
use super::overlay::{DiffLineKind, Overlay};
use super::theme;

pub fn render(frame: &mut Frame, app: &mut App) {
    // Grow/shrink the composer from content before computing layout.
    let width_guess = frame.area().width.saturating_mul(72) / 100;
    // Account for borders + horizontal padding inside the composer.
    app.composer.reflow(width_guess.saturating_sub(6).max(20));

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

    if app.overlay.is_some() {
        render_overlay(frame, area, app);
    }
}

fn render_header(frame: &mut Frame, area: Rect, app: &App) {
    let line = Line::from(vec![
        Span::styled(format!(" {}", app.header.brand), theme::accent_bold()),
        Span::styled(
            format!("   {}   {}   {}", app.header.mode.as_str(), app.header.model, app.header.workspace),
            theme::body(),
        ),
        Span::styled(format!("  {} ", app.header.edit_policy), theme::muted()),
    ]);

    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(theme::border());
    let inner = theme::pad(block.inner(area), 1, 0);
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(line).alignment(Alignment::Left), inner);
}

fn render_conversation(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::border())
        .title(Span::styled(" Conversation ", theme::muted()));
    let inner = theme::pad(block.inner(area), 1, 1);
    frame.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();
    for item in &app.conversation.items {
        match item {
            TimelineItem::User(text) => {
                lines.push(Line::from(Span::styled("You", theme::accent_bold())));
                push_wrapped(&mut lines, text, theme::body());
                lines.push(Line::from(""));
            }
            TimelineItem::Assistant(text) => {
                lines.push(Line::from(Span::styled("Kairo", theme::accent_bold())));
                push_wrapped(&mut lines, text, theme::body());
                lines.push(Line::from(""));
            }
            TimelineItem::Activity { title, lines: body } => {
                lines.push(Line::from(Span::styled(title.clone(), theme::accent_bold())));
                for row in body {
                    lines.push(Line::from(Span::styled(row.clone(), theme::muted())));
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
                lines.push(Line::from(Span::styled(
                    format!("╭─ {title}"),
                    theme::accent_bold(),
                )));
                lines.push(Line::from(Span::styled("│", theme::muted())));
                lines.push(Line::from(vec![
                    Span::styled("│ ", theme::muted()),
                    Span::styled(summary.clone(), theme::body()),
                ]));
                lines.push(Line::from(Span::styled("│", theme::muted())));
                lines.push(Line::from(vec![
                    Span::styled("│ ", theme::muted()),
                    Span::styled("Changed", theme::accent()),
                ]));
                for c in changed {
                    lines.push(Line::from(vec![
                        Span::styled("│ ", theme::muted()),
                        Span::styled(c.clone(), theme::body()),
                    ]));
                }
                lines.push(Line::from(Span::styled("│", theme::muted())));
                lines.push(Line::from(vec![
                    Span::styled("│ ", theme::muted()),
                    Span::styled("Verification", theme::accent()),
                ]));
                for v in verification {
                    lines.push(Line::from(vec![
                        Span::styled("│ ", theme::muted()),
                        Span::styled(v.clone(), theme::body()),
                    ]));
                }
                lines.push(Line::from(Span::styled("│", theme::muted())));
                lines.push(Line::from(vec![
                    Span::styled("│ ", theme::muted()),
                    Span::styled(footer.clone(), theme::muted()),
                ]));
                lines.push(Line::from(Span::styled("╰─", theme::accent())));
                lines.push(Line::from(""));
            }
        }
    }

    if app.run.busy || !app.run.milestones.is_empty() || app.run.current.is_some() {
        lines.push(Line::from(Span::styled("Activity", theme::accent_bold())));
        for row in &app.run.milestones {
            lines.push(Line::from(Span::styled(row.clone(), theme::muted())));
        }
        if let Some(current) = &app.run.current {
            let spin = spinner_glyph(app.run.spinner_frame);
            lines.push(Line::from(Span::styled(
                format!("{spin} {current}"),
                theme::accent(),
            )));
        }
        lines.push(Line::from(""));
    }

    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((app.conversation.scroll, 0)),
        inner,
    );
}

fn push_wrapped(lines: &mut Vec<Line>, text: &str, style: Style) {
    for paragraph in text.split('\n') {
        lines.push(Line::from(Span::styled(paragraph.to_string(), style)));
    }
}

fn render_sidebar(frame: &mut Frame, area: Rect, app: &App, mode: WidthMode) {
    let tabs = tab_line(app.sidebar.tab, mode);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::border())
        .title(tabs);
    let inner = theme::pad(block.inner(area), 1, 1);
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
            match tab {
                SidebarTab::Plan => "Plan",
                SidebarTab::Changes => "Diff",
                SidebarTab::Context => "Info",
            }
        } else {
            tab.title()
        };
        if *tab == active {
            spans.push(Span::styled(format!("[{label}]"), theme::tab_active()));
        } else {
            spans.push(Span::styled(format!(" {label} "), theme::tab_inactive()));
        }
    }
    Line::from(spans)
}

fn render_plan_lines(app: &App) -> Vec<Line<'static>> {
    if app.sidebar.plan_steps.is_empty() {
        return vec![
            Line::from(Span::styled("PLAN", theme::accent_bold())),
            Line::from(""),
            Line::from(Span::styled(
                "No plan yet — it appears when the agent drafts one.",
                theme::muted(),
            )),
        ];
    }
    let mut lines = vec![Line::from(Span::styled(
        format!("PLAN  {}/{}", app.sidebar.plan_done, app.sidebar.plan_total),
        theme::accent_bold(),
    ))];
    lines.push(Line::from(""));
    for step in &app.sidebar.plan_steps {
        let (mark, style) = match step.status {
            PlanStepStatus::Done => ("✓", theme::muted()),
            PlanStepStatus::Active => ("●", theme::accent_bold()),
            PlanStepStatus::Todo => ("○", theme::muted()),
        };
        lines.push(Line::from(Span::styled(
            format!("{mark} {}", step.text),
            style,
        )));
    }
    lines
}

fn render_changes_lines(app: &App) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(Span::styled("CHANGES", theme::accent_bold())),
        Line::from(""),
    ];
    if app.sidebar.changes.is_empty() {
        lines.push(Line::from(Span::styled(
            "No file changes yet.",
            theme::muted(),
        )));
    } else {
        for (i, change) in app.sidebar.changes.iter().enumerate() {
            let selected = i == app.sidebar.selected_change;
            let marker = if selected { ">" } else { " " };
            let style = if selected {
                theme::accent_bold()
            } else {
                theme::body()
            };
            lines.push(Line::from(Span::styled(
                format!("{marker} M {}", change.path),
                style,
            )));
            lines.push(Line::from(Span::styled(
                format!("    {}", change.stats),
                theme::muted(),
            )));
            lines.push(Line::from(""));
        }
        lines.push(Line::from(Span::styled(
            "Enter  View diff",
            theme::muted(),
        )));
    }
    lines
}

fn render_context_lines(app: &App) -> Vec<Line<'static>> {
    let s = &app.sidebar.session;
    vec![
        Line::from(Span::styled("SESSION", theme::accent_bold())),
        Line::from(""),
        kv("Mode", &s.mode),
        kv("Policy", &s.policy),
        kv("Model", &s.model),
        kv("Workspace", &s.workspace),
        kv("Session", &s.session_id),
        kv("Tokens", &s.tokens),
        kv("Elapsed", &s.elapsed),
    ]
}

fn kv(key: &str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{key:<10} "), theme::muted()),
        Span::styled(value.to_string(), theme::body()),
    ])
}

fn render_narrow_plan_strip(frame: &mut Frame, area: Rect, app: &App) {
    let active = app
        .sidebar
        .plan_steps
        .iter()
        .find(|s| s.status == PlanStepStatus::Active)
        .map(|s| s.text.as_str())
        .unwrap_or("…");
    let text = Line::from(vec![
        Span::styled(
            format!(" Plan {}/{} · ", app.sidebar.plan_done, app.sidebar.plan_total),
            theme::muted(),
        ),
        Span::styled(active.to_string(), theme::accent()),
    ]);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::border());
    let inner = theme::pad(block.inner(area), 1, 0);
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(text), inner);
}

fn render_composer(frame: &mut Frame, area: Rect, app: &App) {
    let title = if app.run.busy {
        " Input (busy — Esc cancel) "
    } else {
        " Input · Enter send · Shift+Enter newline "
    };
    let border = if app.run.busy {
        theme::border()
    } else {
        theme::border_focus()
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border)
        .title(Span::styled(title, theme::muted()));
    let inner = theme::pad(block.inner(area), 1, 1);
    frame.render_widget(block, area);

    if app.run.busy {
        frame.render_widget(
            Paragraph::new(Span::styled("Agent is working…", theme::muted_dim())),
            inner,
        );
        return;
    }

    if app.composer.text.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(
                app.composer.placeholder.clone(),
                theme::muted_dim(),
            )),
            inner,
        );
        frame.set_cursor_position((inner.x, inner.y));
        return;
    }

    let scroll = app.composer.scroll as u16;
    frame.render_widget(
        Paragraph::new(app.composer.text.as_str())
            .style(theme::body())
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        inner,
    );

    let (row, col) = app.composer.cursor_row_col(inner.width.max(1));
    let screen_row = row.saturating_sub(app.composer.scroll);
    let x = inner
        .x
        .saturating_add(col as u16)
        .min(inner.x.saturating_add(inner.width.saturating_sub(1)));
    let y = inner
        .y
        .saturating_add(screen_row as u16)
        .min(inner.y.saturating_add(inner.height.saturating_sub(1)));
    frame.set_cursor_position((x, y));
}

fn render_status(frame: &mut Frame, area: Rect, app: &App, mode: WidthMode) {
    let mode_label = match mode {
        WidthMode::Wide => "wide",
        WidthMode::Medium => "medium",
        WidthMode::Narrow => "narrow",
    };
    let live = if app.run.busy {
        if let Some(started) = app.run.started_at {
            let ms = started.elapsed().as_millis() as u64;
            format!(" · {}", feed::format_duration_ms(ms))
        } else {
            String::new()
        }
    } else {
        String::new()
    };
    let text = format!(" {}  ·  layout:{mode_label}{live} ", app.status.hint);
    frame.render_widget(Paragraph::new(text).style(theme::muted_dim()), area);
}

fn render_overlay(frame: &mut Frame, area: Rect, app: &App) {
    match app.overlay.as_ref() {
        Some(Overlay::Approval(view)) => render_approval_modal(frame, area, view),
        Some(Overlay::Diff(view)) => render_diff_modal(frame, area, view),
        Some(Overlay::Help) => render_help_modal(frame, area),
        None => {}
    }
}

fn render_approval_modal(frame: &mut Frame, area: Rect, view: &super::overlay::ApprovalView) {
    let popup = centered_rect(62, 55, area);
    frame.render_widget(Clear, popup);

    let req = &view.request;
    let kind = match req.kind {
        crate::api::ApprovalKind::FileEdit => "file edit",
        crate::api::ApprovalKind::Destructive => "destructive",
        crate::api::ApprovalKind::ExternalEffect => "external effect",
    };

    let mut lines = vec![
        Line::from(Span::styled("Kairo wants to modify:", theme::body())),
        Line::from(""),
        Line::from(Span::styled(format!("  {}", req.summary), theme::accent_bold())),
        Line::from(Span::styled(format!("  ({kind})"), theme::muted())),
        Line::from(""),
    ];
    if !req.reason.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("  {}", req.reason),
            theme::muted(),
        )));
        lines.push(Line::from(""));
    }
    for path in &req.paths {
        lines.push(Line::from(Span::styled(
            format!("  M {}", path.display()),
            theme::body(),
        )));
    }
    if req.paths.is_empty() {
        lines.push(Line::from(Span::styled("  (no paths listed)", theme::muted())));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("[a] Approve once", theme::accent())));
    lines.push(Line::from(Span::styled("[t] Approve for task", theme::accent())));
    lines.push(Line::from(Span::styled("[d] View diff", theme::body())));
    lines.push(Line::from(Span::styled("[r] Reject", theme::muted())));

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::border_focus())
        .title(Span::styled(" Approve changes ", theme::accent_bold()));
    let inner = theme::pad(block.inner(popup), 1, 1);
    frame.render_widget(block, popup);
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn render_diff_modal(frame: &mut Frame, area: Rect, view: &super::overlay::DiffView) {
    let popup = centered_rect(90, 80, area);
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::border_focus())
        .title(Span::styled(
            format!(" {} ", view.title),
            theme::accent_bold(),
        ))
        .title_bottom(Span::styled(" ↑↓ scroll   Esc close ", theme::muted()));
    let inner = theme::pad(block.inner(popup), 1, 0);
    frame.render_widget(block, popup);

    let lines: Vec<Line> = view
        .lines
        .iter()
        .map(|line| {
            let style = match line.kind {
                DiffLineKind::Add => theme::accent(),
                DiffLineKind::Delete => theme::muted(),
                DiffLineKind::Meta => theme::muted_dim(),
                DiffLineKind::Context => theme::body(),
            };
            Line::from(Span::styled(line.text.clone(), style))
        })
        .collect();

    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((view.scroll, 0)),
        inner,
    );
}

fn render_help_modal(frame: &mut Frame, area: Rect) {
    let popup = centered_rect(50, 60, area);
    frame.render_widget(Clear, popup);

    let lines = vec![
        Line::from(Span::styled("/new          New session", theme::body())),
        Line::from(Span::styled("Enter         Send task", theme::body())),
        Line::from(Span::styled("Shift+Enter   Newline", theme::body())),
        Line::from(Span::styled("Esc           Cancel / close", theme::body())),
        Line::from(Span::styled("Ctrl+P/D/I    Plan / Changes / Context", theme::body())),
        Line::from(Span::styled("Ctrl+L        Clear conversation", theme::body())),
        Line::from(Span::styled("a / t / r     Approve once / task / reject", theme::body())),
        Line::from(Span::styled("d             Diff (from approval)", theme::body())),
        Line::from(Span::styled("?             This help", theme::body())),
        Line::from(""),
        Line::from(Span::styled("Esc close", theme::muted())),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::border_focus())
        .title(Span::styled(" Commands ", theme::accent_bold()));
    let inner = theme::pad(block.inner(popup), 1, 1);
    frame.render_widget(block, popup);
    frame.render_widget(Paragraph::new(lines), inner);
}

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
