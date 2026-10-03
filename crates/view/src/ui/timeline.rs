//! What happened, newest first: incidents and events, deployments stand out.

use super::{Theme, block, event, local, short};
use crate::app::{App, Screen, WINDOWS};
use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table, TableState};
use skym_core::model::EventKind;
use skym_core::view::TimelineKind;

pub fn draw(f: &mut Frame, area: Rect, app: &App, theme: Theme) {
    let Screen::Timeline { host, workload, window } = &app.frame().screen else { return };
    let Some(t) = &app.timeline.value else {
        return f.render_widget(Paragraph::new(" loading…"), area);
    };
    let rows: Vec<Row> = t
        .entries
        .iter()
        .map(|e| {
            let (what, style) = match &e.entry {
                TimelineKind::IncidentOpened { code, severity, detail } => (
                    vec![
                        Span::raw("opened   "),
                        theme.severity(*severity),
                        Span::raw(format!(" {} · {detail}", code.as_str())),
                    ],
                    Style::new(),
                ),
                TimelineKind::IncidentReopened { code, severity, detail } => (
                    vec![
                        Span::raw("reopened "),
                        theme.severity(*severity),
                        Span::raw(format!(" {} · {detail}", code.as_str())),
                    ],
                    Style::new(),
                ),
                TimelineKind::IncidentResolved { code } => (
                    vec![Span::raw(format!("resolved   {}", code.as_str()))],
                    theme.fg(Color::Green),
                ),
                TimelineKind::SeverityChanged { code, severity } => (
                    vec![
                        Span::raw("severity "),
                        theme.severity(*severity),
                        Span::raw(format!(" {}", code.as_str())),
                    ],
                    theme.dim(),
                ),
                TimelineKind::Event { event: kind } => {
                    let loud =
                        matches!(kind, EventKind::Deployed { .. } | EventKind::ConfigChanged);
                    (
                        vec![Span::raw(event(kind))],
                        if loud { theme.fg(Color::Cyan) } else { Style::new() },
                    )
                }
                TimelineKind::Unknown => {
                    (vec![Span::raw("(an entry this version does not know)")], theme.dim())
                }
            };
            Row::new([
                Cell::from(local(e.ts, "%m-%d %H:%M")),
                Cell::from(short(&e.subject)),
                Cell::from(Line::from(what)),
            ])
            .style(style)
        })
        .collect();
    let scope =
        workload.as_ref().map_or(host.clone(), |k| format!("{host} › {}/{}", k.project, k.service));
    let more = if t.truncated { " · more not shown" } else { "" };
    let title = format!(" Timeline · {scope} · {}{more} ", WINDOWS[*window]);
    let rows = match rows.is_empty() {
        true => vec![
            Row::new([Cell::from(""), Cell::from(""), Cell::from("nothing in this window")])
                .style(theme.dim()),
        ],
        false => rows,
    };
    let widths = [Constraint::Length(12), Constraint::Percentage(25), Constraint::Fill(1)];
    let mut state = TableState::default().with_selected(Some(app.frame().cursor));
    let table = Table::new(rows, widths)
        .block(block(title, theme))
        .row_highlight_style(theme.selected(true));
    f.render_stateful_widget(table, area, &mut state);
}
