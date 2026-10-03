//! Hosts on the left, every problem on the right: new ones first.

use super::{Theme, age, block, reason, service, short};
use crate::app::{App, Pane};
use jiff::Timestamp;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table, TableState};
use skym_core::rules::Severity;

pub fn draw(f: &mut Frame, area: Rect, app: &App, now: Timestamp, theme: Theme) {
    let [left, right] =
        Layout::horizontal([Constraint::Length(32), Constraint::Min(40)]).areas(area);
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
    let ids = app.host_ids();
    for c in app.overview.value.iter().flat_map(|o| &o.customers) {
        lines.push(Line::styled(c.name.clone(), theme.dim()));
        for h in c.hosts.iter().filter(|h| ids.contains(&&h.id)) {
            let index = 1 + ids.iter().position(|id| **id == h.id).unwrap_or(0);
            let selected = index == app.host_cursor;
            if selected {
                picked_line = lines.len();
            }
            let reported = h.last_report_ago.clone().unwrap_or_else(|| "never".into());
            lines.push(
                Line::from(vec![
                    Span::raw(mark(selected)),
                    theme.status(h.status),
                    Span::raw(format!(" {:<10} {:>8}", h.id, reported)),
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

fn problem_list(f: &mut Frame, area: Rect, app: &App, now: Timestamp, theme: Theme) {
    let focused = app.pane == Pane::Problems;
    let groups = app.problem_groups(now);
    // Tables have no spanning cells: section lines put their text in a wide enough column.
    let header = |text: &str, color: Color| {
        let style = theme.fg(color).add_modifier(Modifier::BOLD);
        Row::new([Cell::from(""), Cell::from(Line::styled(text.to_string(), style))])
    };
    let note = |text: String| {
        Row::new([Cell::from(""), Cell::from(""), Cell::from(text)]).style(theme.dim())
    };
    // A service's own name, unless another service on the same host shares it.
    let all: Vec<_> = groups.new.iter().chain(&groups.ongoing).chain(&groups.info).collect();
    let name = |r: &crate::problems::Row| {
        let twins = all
            .iter()
            .filter(|o| {
                o.host == r.host && service(&o.incident.subject) == service(&r.incident.subject)
            })
            .count();
        if twins > 1 { short(&r.incident.subject) } else { service(&r.incident.subject) }
    };
    let row = |r: &crate::problems::Row| {
        let reason = match &r.incident.mute_reason {
            Some(why) if r.incident.muted => format!("muted: {why}"),
            _ => reason(&r.incident.detail),
        };
        let quiet = r.incident.muted || r.incident.severity == Severity::Info;
        Row::new([
            Cell::from(Line::from(vec![Span::raw(" "), theme.severity(r.incident.severity)])),
            Cell::from(r.host.to_string()),
            Cell::from(name(r)),
            Cell::from(reason),
            Cell::from(Line::from(age(r.age)).right_aligned()),
        ])
        .style(if quiet { theme.dim() } else { Style::new() })
    };
    // Section headers are rows too: the selection skips them.
    let mut table = vec![header("NEW", Color::Red)];
    let mut selectable = Vec::new();
    if groups.new.is_empty() {
        table.push(Row::new([Cell::from(""), Cell::from("(none)")]).style(theme.dim()));
    }
    for r in &groups.new {
        selectable.push(table.len());
        table.push(row(r));
    }
    table.push(header("ONGOING", Color::Reset));
    for r in &groups.ongoing {
        selectable.push(table.len());
        table.push(row(r));
    }
    match (app.show_info, groups.info.len()) {
        (_, 0) => {}
        (true, _) => {
            table.push(header("HYGIENE", Color::DarkGray));
            for r in &groups.info {
                selectable.push(table.len());
                table.push(row(r));
            }
        }
        (false, n) => table.push(note(format!("· {n} hygiene items (h)"))),
    }
    let selected = focused.then(|| selectable.get(app.frame().cursor).copied()).flatten();
    let scope = app.picked_host().map_or("all hosts".to_string(), |h| h.clone());
    let title = format!(" Problems · {scope}{} ", if focused { " ▪" } else { "" });
    let widths = [
        Constraint::Length(3),
        Constraint::Length(8),
        Constraint::Length(26),
        Constraint::Fill(1),
        Constraint::Length(9),
    ];
    let mut state = TableState::default().with_selected(selected);
    let table = Table::new(table, widths).block(block(title, theme));
    f.render_stateful_widget(table.row_highlight_style(theme.selected(true)), area, &mut state);
}
