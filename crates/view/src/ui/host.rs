//! One host: what it is, how it is doing, its problems and its services.

use super::{Theme, ago, block, local, problem_table};
use crate::app::App;
use crate::names::short;
use jiff::Timestamp;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table, TableState};
use skym_core::model::RunState;
use skym_core::view::{HostView, Status};

pub fn draw(f: &mut Frame, area: Rect, app: &App, now: Timestamp, theme: Theme) {
    let Some(h) = &app.host.value else {
        return f.render_widget(Paragraph::new(" loading…"), area);
    };
    let head = header(h, now, theme);
    let (problems, height) =
        problem_table(app, &h.incidents, h.observed_since, now, |i| short(&i.subject), theme);
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

fn header(h: &HostView, now: Timestamp, theme: Theme) -> Vec<Line<'static>> {
    let gb = |b: u64| format!("{:.1} GB", b as f64 / 1e9);
    let watched = h
        .observed_since
        .map_or(String::new(), |t| format!("   watched since {}", local(t, "%m-%d %H:%M")));
    let reported =
        h.last_report_ago.clone().map_or("never reported".into(), |a| format!("report {a} ago"));
    let mut lines = vec![Line::from(vec![
        Span::raw(format!(" {}  ", h.id)),
        theme.status(h.status),
        Span::raw(format!(" {:?}   {reported}{watched}", h.status).to_lowercase()),
    ])];
    if let Some(facts) = &h.facts {
        let docker =
            facts.docker_version.as_deref().map_or(String::new(), |v| format!(" · docker {v}"));
        lines.push(Line::from(format!(
            " {} · kernel {} · {} cpu · {} · up {}{docker}",
            facts.os,
            facts.kernel,
            facts.cpu_count,
            gb(facts.memory_total_bytes),
            ago(now, facts.boot_time)
        )));
    }
    if let Some(state) = &h.state {
        let mut spans = vec![Span::raw(format!(
            " load {:.2} {:.2} {:.2}   memory {} / {}   disks",
            state.load_1m,
            state.load_5m,
            state.load_15m,
            gb(state.memory_used_bytes),
            h.facts.as_ref().map_or("?".into(), |f| gb(f.memory_total_bytes)),
        ))];
        for m in &state.mounts {
            let used = (m.used_bytes * 100).checked_div(m.total_bytes).unwrap_or(0);
            let style = if used >= 85 { theme.fg(Color::Yellow) } else { Style::new() };
            spans.push(Span::styled(format!("  {} {used}%", m.path), style));
        }
        lines.push(Line::from(spans));
    }
    if !h.errors.is_empty() {
        lines.push(Line::styled(
            format!(" collection errors: {}", h.errors.join("; ")),
            theme.fg(Color::Yellow),
        ));
    }
    lines
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
            Row::new([
                Cell::from(Line::from(vec![Span::raw(" "), theme.status(w.status)])),
                Cell::from(format!("{}/{}", w.key.project, w.key.service)),
                Cell::from(run),
                Cell::from(Line::from(since).right_aligned()),
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
        Constraint::Percentage(30),
        Constraint::Length(12),
        Constraint::Length(8),
        Constraint::Fill(1),
        Constraint::Length(9),
    ];
    let title = format!(" Services ({}) · problems first ", services.len());
    let mut state = TableState::default().with_selected(Some(app.frame().cursor));
    let table = Table::new(rows, widths)
        .block(block(title, theme))
        .row_highlight_style(theme.selected(true));
    f.render_stateful_widget(table, area, &mut state);
}
