//! The problems tab: every open problem, new ones first, each named by its application.

use super::{Theme, age, block, empty_row, reason};
use crate::app::App;
use crate::names;
use crate::problems::Row;
use jiff::Timestamp;
use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Row as TableRow, Table, TableState};
use skym_core::rules::Severity;

pub fn draw(f: &mut Frame, area: Rect, app: &App, now: Timestamp, theme: Theme) {
    let groups = app.problem_groups(now);
    // Tables have no spanning cells: section lines put their text in the app column.
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
            table.push(problem_row(app, r, theme));
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
    let widths = [
        Constraint::Length(3),
        Constraint::Length(app_width(area.width)),
        Constraint::Length(8),
        Constraint::Fill(1),
        Constraint::Length(9),
    ];
    let selected = selectable.get(app.frame().cursor).copied();
    let mut state = TableState::default().with_selected(selected);
    let table = Table::new(table, widths).block(block(" Problems ".into(), theme));
    f.render_stateful_widget(table.row_highlight_style(theme.selected(true)), area, &mut state);
}

/// One problem: its application (or host), where, what and why, and how long.
fn problem_row(app: &App, r: &Row, theme: Theme) -> TableRow<'static> {
    let why = match &r.incident.mute_reason {
        Some(why) if r.incident.muted => format!("muted: {why}"),
        _ => reason(&r.incident.detail),
    };
    let what = names::what(&r.incident.subject);
    let text = if what.is_empty() { why } else { format!("{what}: {why}") };
    let quiet = r.incident.muted || r.incident.severity == Severity::Info;
    TableRow::new([
        Cell::from(Line::from(vec![Span::raw(" "), theme.severity(r.incident.severity)])),
        Cell::from(app.app_name(r)),
        Cell::from(r.host.to_string()),
        Cell::from(text),
        Cell::from(Line::from(age(r.age)).right_aligned()),
    ])
    .style(if quiet { theme.dim() } else { Style::new() })
}

/// The app column: at least 12 (names always show), up to 22 while what and why keep 36.
fn app_width(pane: u16) -> u16 {
    const FIXED: u16 = 2 + 3 + 8 + 9 + 4; // borders, columns and the gaps between them
    pane.saturating_sub(FIXED + 36).clamp(12, 22)
}
