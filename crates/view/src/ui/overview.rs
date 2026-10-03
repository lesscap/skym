//! Hosts on the left, every problem on the right: new ones first.

use super::{Theme, age, block, empty_row, reason};
use crate::app::{App, Pane};
use crate::names::{service, short, target};
use crate::problems::Row;
use jiff::Timestamp;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row as TableRow, Table, TableState};
use skym_core::rules::Severity;
use skym_core::subject::Subject;

pub fn draw(f: &mut Frame, area: Rect, app: &App, now: Timestamp, theme: Theme) {
    let [left, right] =
        Layout::horizontal([Constraint::Length(26), Constraint::Min(40)]).areas(area);
    hosts(f, left, app, theme);
    problem_list(f, right, app, now, theme);
}

fn hosts(f: &mut Frame, area: Rect, app: &App, theme: Theme) {
    let focused = app.pane == Pane::Hosts;
    let mark = |selected: bool| if selected && focused { "▸ " } else { "  " };
    let style = |selected: bool| if selected { theme.selected(focused) } else { Style::new() };
    let mut lines = vec![Line::styled(
        format!("{}All hosts", mark(app.host_cursor == 0)),
        style(app.host_cursor == 0),
    )];
    let mut picked_line = 0;
    let targets = app.targets();
    for c in app.overview.value.iter().flat_map(|o| &o.customers) {
        lines.push(Line::styled(c.name.clone(), theme.dim()));
        let hosts = c.hosts.iter().map(|h| {
            let reported = h.last_report_ago.clone().unwrap_or_else(|| "never".into());
            (Subject::Host(h.id.clone()), h.status, reported)
        });
        let endpoints = c.endpoints.iter().map(|e| {
            let answered = match (&e.last_probe_ago, e.latency_ms) {
                (None, _) => "never".to_string(),
                (Some(_), Some(ms)) => latency(ms),
                (Some(_), None) => "—".to_string(),
            };
            (Subject::Endpoint(e.url.clone()), e.status, answered)
        });
        for (subject, status, right) in hosts.chain(endpoints) {
            let Some(index) = targets.iter().position(|t| *t == subject) else { continue };
            let selected = index + 1 == app.host_cursor;
            if selected {
                picked_line = lines.len();
            }
            lines.push(
                Line::from(vec![
                    Span::raw(mark(selected)),
                    theme.status(status),
                    Span::raw(format!(" {:<11} {:>7}", cut(&target(&subject), 11), right)),
                ])
                .style(style(selected)),
            );
        }
    }
    let height = area.height.saturating_sub(2) as usize;
    let scroll = picked_line.saturating_sub(height.saturating_sub(1)) as u16;
    let title = if focused { " Hosts ▪ " } else { " Hosts " };
    f.render_widget(
        Paragraph::new(lines).scroll((scroll, 0)).block(block(title.into(), theme)),
        area,
    );
}

/// `840ms`, `1.2s`.
fn latency(ms: u64) -> String {
    if ms < 1000 { format!("{ms}ms") } else { format!("{:.1}s", ms as f64 / 1000.0) }
}

/// At most `width` characters, the last one `…` when cut.
fn cut(text: &str, width: usize) -> String {
    match text.chars().count() > width {
        true => text.chars().take(width - 1).chain(['…']).collect(),
        false => text.to_string(),
    }
}

fn problem_list(f: &mut Frame, area: Rect, app: &App, now: Timestamp, theme: Theme) {
    let focused = app.pane == Pane::Problems;
    let groups = app.problem_groups(now);
    let all: Vec<&Row<'_>> = groups.new.iter().chain(&groups.ongoing).chain(&groups.info).collect();
    let row = |r: &Row| problem_row(r, &all, theme);
    // Tables have no spanning cells: section lines put their text in a wide enough column.
    let header = |text: &str, color: Color| {
        let style = theme.fg(color).add_modifier(Modifier::BOLD);
        TableRow::new([Cell::from(""), Cell::from(Line::styled(text.to_string(), style))])
    };
    // Section headers are rows too: the selection skips them.
    let mut table = vec![header("NEW", Color::Red)];
    let mut selectable = Vec::new();
    if groups.new.is_empty() {
        table.push(empty_row(1, "(none)".into(), theme));
    }
    let mut add = |table: &mut Vec<TableRow>, rows: &[Row]| {
        for r in rows {
            selectable.push(table.len());
            table.push(row(r));
        }
    };
    add(&mut table, &groups.new);
    table.push(header("ONGOING", Color::Reset));
    add(&mut table, &groups.ongoing);
    match (app.show_info, groups.info.len()) {
        (_, 0) => {}
        (true, _) => {
            table.push(header("HYGIENE", Color::DarkGray));
            add(&mut table, &groups.info);
        }
        (false, n) => table.push(empty_row(3, format!("· {n} hygiene items (h)"), theme)),
    }
    let selected = focused.then(|| selectable.get(app.frame().cursor).copied()).flatten();
    let scope = app.picked().map_or("all hosts".to_string(), |t| target(&t));
    let title = format!(" Problems · {scope}{} ", if focused { " ▪" } else { "" });
    let widths = [
        Constraint::Length(3),
        Constraint::Length(8), // holds the section headers too
        Constraint::Length(name_width(area.width)),
        Constraint::Fill(1),
        Constraint::Length(9),
    ];
    let mut state = TableState::default().with_selected(selected);
    let table = Table::new(table, widths).block(block(title, theme));
    f.render_stateful_widget(table.row_highlight_style(theme.selected(true)), area, &mut state);
}

/// One problem: a service by its own name unless another on the same host shares it.
fn problem_row(r: &Row, all: &[&Row], theme: Theme) -> TableRow<'static> {
    let same_name = |o: &&&Row| {
        o.host == r.host && service(&o.incident.subject) == service(&r.incident.subject)
    };
    let name = match all.iter().filter(same_name).count() {
        1 => service(&r.incident.subject),
        _ => short(&r.incident.subject),
    };
    let why = match &r.incident.mute_reason {
        Some(why) if r.incident.muted => format!("muted: {why}"),
        _ => reason(&r.incident.detail),
    };
    let quiet = r.incident.muted || r.incident.severity == Severity::Info;
    TableRow::new([
        Cell::from(Line::from(vec![Span::raw(" "), theme.severity(r.incident.severity)])),
        Cell::from(r.host.to_string()),
        Cell::from(name),
        Cell::from(why),
        Cell::from(Line::from(age(r.age)).right_aligned()),
    ])
    .style(if quiet { theme.dim() } else { Style::new() })
}

/// The name column: at least 12 (names always show), up to 26 while the reason keeps 20.
fn name_width(pane: u16) -> u16 {
    const FIXED: u16 = 2 + 3 + 8 + 9 + 4; // borders, columns and the gaps between them
    pane.saturating_sub(FIXED + 20).clamp(12, 26)
}
