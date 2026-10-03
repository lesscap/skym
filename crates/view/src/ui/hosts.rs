//! The hosts tab: every host, how loaded it is, and how its applications are doing.

use super::{Theme, block, empty_row};
use crate::app::App;
use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table, TableState};
use skym_core::view::{DiskUse, HostOverview, Status};

pub fn draw(f: &mut Frame, area: Rect, app: &App, theme: Theme) {
    if app.overview.value.is_none() {
        return f.render_widget(Paragraph::new(" loading…"), area);
    }
    let hosts = app.host_rows();
    let worst = |host: &str| {
        let apps = app.apps.value.iter().flat_map(|l| &l.apps);
        apps.filter(|a| a.key.host == host && a.status != Status::Ok).map(|a| a.status).max()
    };
    let mut rows: Vec<Row> = hosts.iter().map(|h| host_row(h, worst(&h.id), theme)).collect();
    if rows.is_empty() {
        rows.push(empty_row(1, "(none)".into(), theme));
    }
    let widths = [
        Constraint::Length(3),
        Constraint::Length(10),
        Constraint::Length(7),
        Constraint::Length(6),
        Constraint::Length(15),
        Constraint::Fill(1),
        Constraint::Length(10),
    ];
    let title = format!(" Hosts ({}) ", hosts.len());
    let mut state = TableState::default().with_selected(Some(app.frame().cursor));
    let table = Table::new(rows, widths)
        .header(
            Row::new(["", "HOST", "REPORT", "LOAD", "MEMORY", "DISKS", "APPS"]).style(theme.dim()),
        )
        .block(block(title, theme))
        .row_highlight_style(theme.selected(true));
    f.render_stateful_widget(table, area, &mut state);
}

/// `worst`: the worst status among its applications in trouble, for their count's mark.
fn host_row(h: &HostOverview, worst: Option<Status>, theme: Theme) -> Row<'static> {
    let gb = |b: u64| format!("{:.1}", b as f64 / 1e9);
    let memory = match (h.memory_used_bytes, h.memory_total_bytes) {
        (Some(used), Some(total)) => format!("{} / {} GB", gb(used), gb(total)),
        (Some(used), None) => format!("{} GB", gb(used)),
        _ => String::new(),
    };
    let trouble = match (h.apps_in_trouble, worst) {
        (0, _) | (_, None) => vec![],
        (n, Some(status)) => {
            vec![Span::raw(format!("  ({n} ")), theme.status(status), Span::raw(")")]
        }
    };
    Row::new([
        Cell::from(Line::from(vec![Span::raw(" "), theme.status(h.status)])),
        Cell::from(h.id.clone()),
        Cell::from(Line::from(h.last_report_ago.clone().unwrap_or("never".into())).right_aligned()),
        Cell::from(
            Line::from(h.load_1m.map_or(String::new(), |l| format!("{l:.2}"))).right_aligned(),
        ),
        Cell::from(memory),
        Cell::from(Line::from(disks(&h.disks, theme))),
        Cell::from(Line::from([vec![Span::raw(h.apps.to_string())], trouble].concat())),
    ])
}

/// `/ 61%  /data 44% ▲`: yellow from 85%, `▲` while one fills up.
fn disks(disks: &[DiskUse], theme: Theme) -> Vec<Span<'static>> {
    disks
        .iter()
        .map(|d| {
            let mark = if d.filling { " ▲" } else { "" };
            let style = if d.used_percent >= 85 || d.filling {
                theme.fg(Color::Yellow)
            } else {
                Style::new()
            };
            Span::styled(format!("{} {}%{mark}  ", d.path, d.used_percent), style)
        })
        .collect()
}
