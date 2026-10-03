//! Every application, grouped by environment; and one application with its URLs and services.

use super::host::service_table;
use super::{Theme, ago, block, empty_row, local, problem_table};
use crate::app::App;
use crate::names;
use jiff::Timestamp;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table, TableState};
use skym_core::subject::Subject;
use skym_core::view::{AppSummary, EndpointOverview, Status};

pub fn list(f: &mut Frame, area: Rect, app: &App, now: Timestamp, theme: Theme) {
    let Some(all) = &app.apps.value else {
        return f.render_widget(Paragraph::new(" loading…"), area);
    };
    let header = |text: String| {
        let style = Style::new().add_modifier(Modifier::BOLD);
        Row::new([Cell::from(""), Cell::from(Line::styled(text, style))])
    };
    let mut rows = Vec::new();
    let mut selectable = Vec::new();
    for g in app.app_groups() {
        rows.push(header(g.env.unwrap_or("unclassified").to_uppercase()));
        for a in &g.apps {
            selectable.push(rows.len());
            rows.push(app_row(a, now, theme));
        }
        if g.hidden > 0 {
            rows.push(empty_row(1, format!("· {} more, all ok (e)", g.hidden), theme));
        }
    }
    if selectable.is_empty() {
        rows.push(empty_row(1, "(none)".into(), theme));
    }
    let widths = [
        Constraint::Length(3),
        Constraint::Length(22),
        Constraint::Length(6),
        Constraint::Length(5),
        Constraint::Fill(1),
        Constraint::Length(9),
    ];
    let title =
        format!(" Applications ({}){} ", all.apps.len(), if app.all_envs { " · all" } else { "" });
    let selected = selectable.get(app.frame().cursor).copied();
    let mut state = TableState::default().with_selected(selected);
    let table = Table::new(rows, widths)
        .header(Row::new(["", "APP", "HOST", "SVC", "URL", "DEPLOYED"]).style(theme.dim()))
        .block(block(title, theme))
        .row_highlight_style(theme.selected(true));
    f.render_stateful_widget(table, area, &mut state);
}

/// One application: its first URL and how it answers, else what is wrong with it.
fn app_row(a: &AppSummary, now: Timestamp, theme: Theme) -> Row<'static> {
    let url = match (a.endpoints.first(), a.incidents.first()) {
        (Some(e), _) => endpoint_text(e, a.endpoints.len()),
        (None, Some(i)) => super::reason(&i.detail),
        (None, None) => "—".into(),
    };
    let deployed = a.last_deployed.map_or("—".to_string(), |t| ago(now, t));
    Row::new([
        Cell::from(Line::from(vec![Span::raw(" "), theme.status(a.status)])),
        Cell::from(a.name.clone()),
        Cell::from(a.key.host.clone()),
        Cell::from(format!("{}/{}", a.running, a.services)),
        Cell::from(url),
        Cell::from(Line::from(deployed).right_aligned()),
    ])
    .style(if a.status == Status::Ok && a.services == 0 { theme.dim() } else { Style::new() })
}

/// `shop.example.com  84ms`, and how many more URLs there are.
fn endpoint_text(e: &EndpointOverview, count: usize) -> String {
    let answer = match (e.http_status, e.latency_ms) {
        (Some(_), Some(ms)) => format!("{ms}ms"),
        _ if e.last_probe_ago.is_none() => "not probed yet".into(),
        _ => "no answer".into(),
    };
    let more = if count > 1 { format!("  +{}", count - 1) } else { String::new() };
    format!("{}  {answer}{more}", names::short(&Subject::Endpoint(e.url.clone())))
}

pub fn one(f: &mut Frame, area: Rect, app: &App, now: Timestamp, theme: Theme) {
    let Some(v) = &app.app.value else {
        return f.render_widget(Paragraph::new(" loading…"), area);
    };
    let a = &v.app;
    let head = header(a, theme);
    let observed = app.observed_since(&a.key.host);
    let (problems, height) =
        problem_table(app, &a.incidents, observed, now, |i| names::short(&i.subject), theme);
    let [head_area, problems_area, services] = Layout::vertical([
        Constraint::Length(head.len() as u16),
        Constraint::Length(height.min((area.height / 3).max(3))),
        Constraint::Min(3),
    ])
    .areas(area);
    f.render_widget(Paragraph::new(head), head_area);
    f.render_widget(problems, problems_area);
    service_table(f, services, app, now, theme);
}

fn header(a: &AppSummary, theme: Theme) -> Vec<Line<'static>> {
    let env = a.env.as_deref().unwrap_or("unclassified");
    let mut lines = vec![Line::from(vec![
        Span::raw(format!(" {}  ", a.name)),
        theme.status(a.status),
        Span::raw(format!(" {}   {}   {env}", format!("{:?}", a.status).to_lowercase(), a.key)),
    ])];
    if let Some(note) = &a.note {
        lines.push(Line::styled(format!(" {note}"), theme.dim()));
    }
    for e in &a.endpoints {
        let cert = e
            .cert_expires_at
            .map_or(String::new(), |t| format!("   cert until {}", local(t, "%Y-%m-%d")));
        let status = e.http_status.map_or(String::new(), |s| format!("{s} "));
        let answer =
            e.latency_ms.map_or("no answer".to_string(), |ms| format!("{status}in {ms}ms"));
        lines.push(Line::from(vec![
            Span::raw(" "),
            theme.status(e.status),
            Span::raw(format!(" {}   {answer}{cert}", e.url)),
        ]));
    }
    lines
}
