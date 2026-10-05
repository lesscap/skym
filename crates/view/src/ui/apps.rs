//! Every application, grouped by environment; and one application with its URLs and services.

use super::format::{ago, answer, app_count, cores, reason, size, usage};
use super::{Theme, block, draw_preview, empty_row, preview, problem_table, with_preview};
use crate::app::{App, AppRow};
use crate::apps::{Group, Grouping, Sort, used_cpu, used_memory};
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
    let listed = app.app_rows();
    let (area, preview_area) = with_preview(area, app.preview);
    if let Some(at) = preview_area {
        let (title, lines) = match listed.get(app.frame().cursor) {
            Some(AppRow::App(a)) => {
                (format!(" {} · {} ", a.name, a.key.host), preview::application(a, now, theme))
            }
            Some(AppRow::Group(g)) => (format!(" {} ", g.name), preview::group(g, theme)),
            None => (" Preview ".into(), Vec::new()),
        };
        draw_preview(f, at, title, lines, theme);
    }
    let mut rows: Vec<Row> = listed
        .iter()
        .map(|r| match r {
            AppRow::Group(g) => group_row(g, app.grouping, theme),
            AppRow::App(a) => app_row(a, a.key.host.clone(), host_memory(app, a), now, theme),
        })
        .collect();
    if rows.is_empty() {
        rows.push(empty_row(1, "(none)".into(), theme));
    }
    let title = format!(
        " Applications ({}) · by {}{} ",
        all.apps.len(),
        app.grouping.label(),
        if app.sort == Sort::default() {
            String::new()
        } else {
            format!(" · {}", app.sort.label())
        }
    );
    let selected = (!listed.is_empty()).then_some(app.frame().cursor);
    let mut state = TableState::default().with_selected(selected);
    let table = Table::new(rows, app_columns(area.width))
        .header(app_header("HOST", app, theme))
        .block(block(title, theme))
        .row_highlight_style(theme.selected(true));
    f.render_stateful_widget(table, area, &mut state);
}

/// `✗ ▸ PROD  43 apps  9.1G`: its worst status, folded or open, how many it has and the
/// memory they use.
fn group_row(g: &Group, by: Grouping, theme: Theme) -> Row<'static> {
    let worst = match g.worst() {
        Status::Ok => Span::raw(""),
        s => theme.status(s),
    };
    let name = if by == Grouping::Env { g.name.to_uppercase() } else { g.name.clone() };
    let mark = if g.open { "▾" } else { "▸" };
    Row::new([
        Cell::from(Line::from(vec![Span::raw(" "), worst])),
        Cell::from(Line::styled(
            format!("{mark} {name}"),
            Style::new().add_modifier(Modifier::BOLD),
        )),
        Cell::from(Line::styled(app_count(g.all.len()), theme.dim())),
        Cell::from(""),
        Cell::from(""),
        Cell::from(Line::from(g.memory().map_or(String::new(), size)).right_aligned()),
    ])
}

/// Status, name, host or environment, services, CPU, memory, up for, first URL, errors,
/// deployed.
const APP_COLUMNS: [Constraint; 10] = [
    Constraint::Length(3),
    Constraint::Length(22),
    Constraint::Length(9), // `external` fits
    Constraint::Length(5),
    Constraint::Length(5), // `12.5`, cores
    Constraint::Length(9), // `999M 100%` fits
    Constraint::Length(7),
    Constraint::Fill(1),
    Constraint::Length(6),
    Constraint::Length(9),
];

/// The columns for a table `width` wide: below the room for all of them, the deployment
/// time goes first, so names keep their width.
fn app_columns(width: u16) -> [Constraint; 10] {
    let mut columns = APP_COLUMNS;
    if width < 88 {
        columns[9] = Constraint::Length(0);
    }
    columns
}

/// One application's line: `second` is where it runs (in the list) or its environment (on a
/// host). Its memory, with its share of `host_memory`. Then its first URL and how it
/// answers, else what is wrong with it.
fn app_row(
    a: &AppSummary,
    second: String,
    host_memory: Option<u64>,
    now: Timestamp,
    theme: Theme,
) -> Row<'static> {
    let url = match (a.endpoints.first(), a.incidents.first()) {
        (Some(e), _) => endpoint_text(e, a.endpoints.len()),
        (None, Some(i)) => reason(&i.detail),
        (None, None) => "—".into(),
    };
    let deployed = a.last_deployed.map_or("—".to_string(), |t| ago(now, t));
    Row::new([
        Cell::from(Line::from(vec![Span::raw(" "), theme.status(a.status)])),
        Cell::from(a.name.clone()),
        Cell::from(second),
        Cell::from(format!("{}/{}", a.running, a.services)),
        Cell::from(Line::from(used_cpu(a).map_or("—".into(), cores)).right_aligned()),
        Cell::from(Line::from(mem_cell(a, host_memory)).right_aligned()),
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

/// The columns' names: `second` is HOST or ENV; a `▼` on the one the applications are
/// sorted by.
fn app_header(second: &'static str, app: &App, theme: Theme) -> Row<'static> {
    let names = ["", "APP", second, "SVC", "CPU", "MEM", "UP", "URL", "ERR 1H", "DEPLOYED"];
    let sorted = app.sort.column();
    let names = names.map(|n| if Some(n) == sorted { format!("{n} ▼") } else { n.to_string() });
    Row::new(names).style(theme.dim())
}

/// `2.3G 14%` of its host's memory, `2.3G` without the host's total, `—` without any.
fn mem_cell(a: &AppSummary, host_memory: Option<u64>) -> String {
    let Some(used) = used_memory(a) else { return "—".into() };
    match host_memory.filter(|t| *t > 0) {
        Some(total) => format!("{} {}%", size(used), used * 100 / total),
        None => size(used),
    }
}

/// The total memory of the host an application runs on, as the overview has it.
fn host_memory(app: &App, a: &AppSummary) -> Option<u64> {
    app.host_summary(&a.key.host)?.memory_total_bytes
}

/// How long its longest-running workload has been running.
fn up(a: &AppSummary, now: Timestamp) -> String {
    let running = a.workloads.iter().filter(|w| w.run == RunState::Running);
    running.filter_map(|w| w.state_since).min().map_or(String::new(), |t| ago(now, t))
}

/// A host's applications, problems first.
pub(super) fn host_apps(f: &mut Frame, area: Rect, app: &App, now: Timestamp, theme: Theme) {
    let apps = app.host_apps();
    let mut rows: Vec<Row> = apps
        .iter()
        .map(|a| {
            let env = a.env.clone().unwrap_or_default();
            app_row(a, env, host_memory(app, a), now, theme)
        })
        .collect();
    if rows.is_empty() {
        rows.push(empty_row(1, "(none)".into(), theme));
    }
    let widths = app_columns(area.width);
    let order = app.sort.label();
    let title = format!(" Apps ({}) · {order} ", apps.len());
    let mut state = TableState::default().with_selected(Some(app.frame().cursor));
    let table = Table::new(rows, widths)
        .header(app_header("ENV", app, theme))
        .block(block(title, theme))
        .row_highlight_style(theme.selected(true));
    f.render_stateful_widget(table, area, &mut state);
}

/// `shop.example.com  84ms`, and how many more URLs there are.
fn endpoint_text(e: &EndpointOverview, count: usize) -> String {
    let more = if count > 1 { format!("  +{}", count - 1) } else { String::new() };
    let url = names::short(&Subject::Endpoint(e.url.clone()));
    format!("{url}  {}{more}", answer(e, false))
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
            let cpu = w.cpu_cores.map_or(String::new(), cores);
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
                Cell::from(Line::from(cpu).right_aligned()),
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
        Constraint::Length(5),
        Constraint::Length(13),
        Constraint::Length(4),
        Constraint::Fill(1),
        Constraint::Length(9),
    ];
    let title = format!(" Services ({}) · problems first ", services.len());
    let mut state = TableState::default().with_selected(Some(app.frame().cursor));
    let header = ["", "SERVICE", "RUN", "FOR", "CPU", "MEMORY", "RST", "IMAGE", ""];
    let table = Table::new(rows, widths)
        .header(Row::new(header).style(theme.dim()))
        .block(block(title, theme))
        .row_highlight_style(theme.selected(true));
    f.render_stateful_widget(table, area, &mut state);
}
