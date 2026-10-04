//! Every application, grouped by environment; and one application with its URLs and services.

use super::preview::usage;
use super::{Theme, ago, block, draw_preview, empty_row, preview, problem_table, with_preview};
use crate::app::App;
use crate::names;
use jiff::Timestamp;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table, TableState};
use skym_core::model::RunState;
use skym_core::subject::Subject;
use skym_core::view::{AppSummary, EndpointOverview, Status};

pub fn list(f: &mut Frame, area: Rect, app: &App, now: Timestamp, theme: Theme) {
    let Some(all) = &app.apps.value else {
        return f.render_widget(Paragraph::new(" loading…"), area);
    };
    let (area, preview_area) = with_preview(area, app.preview);
    if let Some(at) = preview_area {
        let selected = app.app_rows().get(app.frame().cursor).copied();
        let lines = selected.map_or_else(Vec::new, |a| preview::application(a, now, theme));
        let title =
            selected.map_or(" Preview ".into(), |a| format!(" {} · {} ", a.name, a.key.host));
        draw_preview(f, at, title, lines, theme);
    }
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
            rows.push(app_row(a, a.key.host.clone(), now, theme));
        }
        if g.hidden > 0 {
            rows.push(empty_row(1, format!("· {} more, all ok (e)", g.hidden), theme));
        }
    }
    if selectable.is_empty() {
        rows.push(empty_row(1, "(none)".into(), theme));
    }
    let widths = APP_COLUMNS;
    let title =
        format!(" Applications ({}){} ", all.apps.len(), if app.all_envs { " · all" } else { "" });
    let selected = selectable.get(app.frame().cursor).copied();
    let mut state = TableState::default().with_selected(selected);
    let table = Table::new(rows, widths)
        .header(
            Row::new(["", "APP", "HOST", "SVC", "UP", "URL", "ERR 1H", "DEPLOYED"])
                .style(theme.dim()),
        )
        .block(block(title, theme))
        .row_highlight_style(theme.selected(true));
    f.render_stateful_widget(table, area, &mut state);
}

/// Status, name, host or environment, services, up for, first URL, errors, deployed.
const APP_COLUMNS: [Constraint; 8] = [
    Constraint::Length(3),
    Constraint::Length(22),
    Constraint::Length(9), // `external` fits
    Constraint::Length(5),
    Constraint::Length(7),
    Constraint::Fill(1),
    Constraint::Length(6),
    Constraint::Length(9),
];

/// One application's line: `second` is where it runs (in the list) or its environment (on a
/// host). Then its first URL and how it answers, else what is wrong with it.
fn app_row(a: &AppSummary, second: String, now: Timestamp, theme: Theme) -> Row<'static> {
    let url = match (a.endpoints.first(), a.incidents.first()) {
        (Some(e), _) => endpoint_text(e, a.endpoints.len()),
        (None, Some(i)) => super::reason(&i.detail),
        (None, None) => "—".into(),
    };
    let deployed = a.last_deployed.map_or("—".to_string(), |t| ago(now, t));
    Row::new([
        Cell::from(Line::from(vec![Span::raw(" "), theme.status(a.status)])),
        Cell::from(a.name.clone()),
        Cell::from(second),
        Cell::from(format!("{}/{}", a.running, a.services)),
        Cell::from(Line::from(up(a, now)).right_aligned()),
        Cell::from(url),
        Cell::from(
            Line::from(match a.exceptions_1h {
                0 => String::new(),
                n => n.to_string(),
            })
            .right_aligned(),
        ),
        Cell::from(Line::from(deployed).right_aligned()),
    ])
    .style(if a.status == Status::Ok && a.services == 0 { theme.dim() } else { Style::new() })
}

/// How long its longest-running workload has been running.
fn up(a: &AppSummary, now: Timestamp) -> String {
    let running = a.workloads.iter().filter(|w| w.run == RunState::Running);
    running.filter_map(|w| w.state_since).min().map_or(String::new(), |t| ago(now, t))
}

/// A host's applications, problems first.
pub(super) fn host_apps(f: &mut Frame, area: Rect, app: &App, now: Timestamp, theme: Theme) {
    let apps = app.host_apps();
    let mut rows: Vec<Row> =
        apps.iter().map(|a| app_row(a, a.env.clone().unwrap_or_default(), now, theme)).collect();
    if rows.is_empty() {
        rows.push(empty_row(1, "(none)".into(), theme));
    }
    let widths = APP_COLUMNS;
    let title = format!(" Apps ({}) · problems first ", apps.len());
    let mut state = TableState::default().with_selected(Some(app.frame().cursor));
    let table = Table::new(rows, widths)
        .header(
            Row::new(["", "APP", "ENV", "SVC", "UP", "URL", "ERR 1H", "DEPLOYED"])
                .style(theme.dim()),
        )
        .block(block(title, theme))
        .row_highlight_style(theme.selected(true));
    f.render_stateful_widget(table, area, &mut state);
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
    let mut head = preview::app_head(a, theme);
    head.extend(preview::app_history(a, 5, now, theme));
    let (problems, height) =
        problem_table(app, &a.incidents, now, |i| names::short(&i.subject), theme);
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

pub(super) fn service_table(f: &mut Frame, area: Rect, app: &App, now: Timestamp, theme: Theme) {
    let services = app.services();
    let rows: Vec<Row> = services
        .iter()
        .map(|w| {
            let run = match (w.run, w.exit_code) {
                (RunState::Exited, Some(code)) => format!("exited ({code})"),
                (run, _) => run.to_string(),
            };
            let finished_job = w.run == RunState::Exited && w.exit_code == Some(0);
            let since = w.state_since.map_or(String::new(), |t| ago(now, t));
            let datastore = if w.kind == Some(skym_core::model::WorkloadKind::Datastore) {
                "datastore"
            } else {
                ""
            };
            let memory =
                w.memory_used_bytes.map_or(String::new(), |used| usage(used, w.memory_limit_bytes));
            let restarts = match w.restarts_last_hour {
                0 => String::new(),
                n => format!("{n}×"),
            };
            Row::new([
                Cell::from(Line::from(vec![Span::raw(" "), theme.status(w.status)])),
                Cell::from(format!("{}/{}", w.key.project, w.key.service)),
                Cell::from(run),
                Cell::from(Line::from(since).right_aligned()),
                Cell::from(Line::from(memory).right_aligned()),
                Cell::from(Line::from(restarts).right_aligned()),
                Cell::from(w.image.clone().unwrap_or_default()),
                Cell::from(Span::styled(datastore, theme.dim())),
            ])
            .style(if finished_job && w.status == Status::Ok {
                theme.dim()
            } else {
                Style::new()
            })
        })
        .collect();
    let widths = [
        Constraint::Length(3),
        Constraint::Percentage(25),
        Constraint::Length(12),
        Constraint::Length(8),
        Constraint::Length(13),
        Constraint::Length(4),
        Constraint::Fill(1),
        Constraint::Length(9),
    ];
    let title = format!(" Services ({}) · problems first ", services.len());
    let mut state = TableState::default().with_selected(Some(app.frame().cursor));
    let header = ["", "SERVICE", "RUN", "FOR", "MEMORY", "RST", "IMAGE", ""];
    let table = Table::new(rows, widths)
        .header(Row::new(header).style(theme.dim()))
        .block(block(title, theme))
        .row_highlight_style(theme.selected(true));
    f.render_stateful_widget(table, area, &mut state);
}
