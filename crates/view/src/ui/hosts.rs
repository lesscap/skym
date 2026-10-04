//! The hosts tab: every host, how loaded it is, and how its applications are doing.

use super::preview::{self, memory, size};
use super::{Theme, ago, block, draw_preview, empty_row, with_preview};
use crate::app::App;
use jiff::Timestamp;
use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table, TableState};
use skym_core::view::{DiskUse, HostOverview, Status};

pub fn draw(f: &mut Frame, area: Rect, app: &App, now: Timestamp, theme: Theme) {
    if app.overview.value.is_none() {
        return f.render_widget(Paragraph::new(" loading…"), area);
    }
    let hosts = app.host_rows();
    let (area, preview_area) = with_preview(area, app.preview);
    if let Some(at) = preview_area {
        let selected = hosts.get(app.frame().cursor).copied();
        let lines = selected.map_or_else(Vec::new, |h| preview::host(app, h, now, theme));
        let title = selected.map_or(" Preview ".into(), |h| format!(" {} ", h.id));
        draw_preview(f, at, title, lines, theme);
    }
    let worst = |host: &str| {
        let apps = app.apps.value.iter().flat_map(|l| &l.apps);
        apps.filter(|a| a.key.host == host && a.status != Status::Ok).map(|a| a.status).max()
    };
    let mut rows: Vec<Row> = hosts.iter().map(|h| host_row(h, worst(&h.id), now, theme)).collect();
    if rows.is_empty() {
        rows.push(empty_row(1, "(none)".into(), theme));
    }
    let widths = [
        Constraint::Length(3),
        Constraint::Length(10),
        Constraint::Length(15),
        Constraint::Length(7),
        Constraint::Length(7),
        Constraint::Length(4),
        Constraint::Length(6),
        Constraint::Length(15),
        Constraint::Fill(1),
        Constraint::Length(10),
    ];
    let title = format!(" Hosts ({}) ", hosts.len());
    let mut state = TableState::default().with_selected(Some(app.frame().cursor));
    let header = ["", "HOST", "IP", "REPORT", "UP", "CPU", "LOAD", "MEMORY", "DISKS", "APPS"];
    let table = Table::new(rows, widths)
        .header(Row::new(header).style(theme.dim()))
        .block(block(title, theme))
        .row_highlight_style(theme.selected(true));
    f.render_stateful_widget(table, area, &mut state);
}

/// `worst`: the worst status among its applications in trouble, for their count's mark.
fn host_row(h: &HostOverview, worst: Option<Status>, now: Timestamp, theme: Theme) -> Row<'static> {
    let trouble = match (h.apps_in_trouble, worst) {
        (0, _) | (_, None) => vec![],
        (n, Some(status)) => {
            vec![Span::raw(format!("  ({n} ")), theme.status(status), Span::raw(")")]
        }
    };
    let right = |text: String| Cell::from(Line::from(text).right_aligned());
    Row::new([
        Cell::from(Line::from(vec![Span::raw(" "), theme.status(h.status)])),
        Cell::from(h.id.clone()),
        Cell::from(h.ip.clone().unwrap_or_default()),
        right(h.last_report_ago.clone().unwrap_or("never".into())),
        right(h.boot_time.map_or(String::new(), |t| ago(now, t))),
        right(h.cpu_count.map_or(String::new(), |n| n.to_string())),
        right(h.load_1m.map_or(String::new(), |l| format!("{l:.2}"))),
        Cell::from(
            h.memory_used_bytes
                .map_or(String::new(), |used| memory(Some(used), h.memory_total_bytes)),
        ),
        Cell::from(Line::from(disks(&h.disks, theme))),
        Cell::from(Line::from([vec![Span::raw(h.apps.to_string())], trouble].concat())),
    ])
}

/// `/ 61% of 40G  /data 44% of 80G ▲`: yellow from 85%, `▲` while one fills up.
fn disks(disks: &[DiskUse], theme: Theme) -> Vec<Span<'static>> {
    // Small system partitions (`/boot/efi`) would crowd out the disks that matter, unless
    // they are the ones filling up.
    disks
        .iter()
        .filter(|d| {
            d.total_bytes == 0
                || d.total_bytes >= 1_000_000_000
                || d.filling
                || d.used_percent >= 85
        })
        .map(|d| {
            let mark = if d.filling { " ▲" } else { "" };
            let style = if d.used_percent >= 85 || d.filling {
                theme.fg(Color::Yellow)
            } else {
                Style::new()
            };
            let of = if d.total_bytes > 0 {
                format!(" of {}", size(d.total_bytes))
            } else {
                String::new()
            };
            Span::styled(format!("{} {}%{of}{mark}  ", d.path, d.used_percent), style)
        })
        .collect()
}
