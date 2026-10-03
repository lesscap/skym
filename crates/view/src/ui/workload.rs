//! One service: its problems, state and facts side by side, exceptions, recent events.
//! Also a host's exceptions, which read the same way.

use super::{Theme, ago, block, empty_row, event, problem_table, reason};
use crate::app::App;
use crate::names::short;
use jiff::Timestamp;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Color;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table, TableState};
use skym_core::model::{ExceptionGroup, Health, RunState, WorkloadFacts, WorkloadState};
use skym_core::subject::Subject;
use skym_core::view::WorkloadView;

pub fn draw(f: &mut Frame, area: Rect, app: &App, now: Timestamp, theme: Theme) {
    let Some(w) = &app.workload.value else {
        return f.render_widget(Paragraph::new(" loading…"), area);
    };
    let observed = app.observed_since(&w.key.host);
    let code = |i: &skym_core::view::IncidentView| i.code.as_str().to_string();
    let (problems, height) = problem_table(app, &w.incidents, observed, now, code, theme);
    let [head, problems_area, details, exceptions_area, events_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(height.min(8)),
        Constraint::Length(8),
        Constraint::Min(4),
        Constraint::Length(8),
    ])
    .areas(area);
    f.render_widget(Paragraph::new(title(w, theme)), head);
    f.render_widget(problems, problems_area);
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)]).areas(details);
    let state = Paragraph::new(state_lines(&w.state, now, theme));
    f.render_widget(state.block(block(" State ".into(), theme)), left);
    let facts = Paragraph::new(fact_lines(w.facts.as_ref()));
    f.render_widget(facts.block(block(" Facts ".into(), theme)), right);
    groups(f, exceptions_area, app, &w.exceptions, " Exceptions · last hour ", now, theme);
    f.render_widget(events(w, now, theme), events_area);
}

fn title(w: &WorkloadView, theme: Theme) -> Line<'static> {
    let name = crate::names::full(&Subject::Workload(w.key.clone()));
    Line::from(vec![
        Span::raw(format!(" {} › {name}  ", w.key.host)),
        theme.status(w.status),
        Span::raw(format!(" {:?}", w.status).to_lowercase()),
    ])
}

fn events(w: &WorkloadView, now: Timestamp, theme: Theme) -> Paragraph<'static> {
    let lines: Vec<Line> = w
        .events
        .iter()
        .rev()
        .map(|e| Line::from(format!(" {:>6} ago  {}", ago(now, e.ts), event(&e.kind))))
        .collect();
    let lines = match lines.is_empty() {
        true => vec![Line::styled(" no events in the last day", theme.dim())],
        false => lines,
    };
    Paragraph::new(lines).block(block(" Events · 24h ".into(), theme))
}

/// A host's exception groups, from `e` on the host screen.
pub fn exceptions(f: &mut Frame, area: Rect, app: &App, host: &str, now: Timestamp, theme: Theme) {
    let list = app.exceptions.value.as_ref().map_or(&[][..], |l| &l.exceptions[..]);
    groups(f, area, app, list, &format!(" {host} · exceptions · last hour "), now, theme);
}

fn groups(
    f: &mut Frame,
    area: Rect,
    app: &App,
    list: &[ExceptionGroup],
    title: &str,
    now: Timestamp,
    theme: Theme,
) {
    // A row taller than the table is not drawn at all: an expanded stack fits or is cut.
    let room = area.height.saturating_sub(2) as usize;
    let rows: Vec<Row> = list
        .iter()
        .enumerate()
        .map(|(i, g)| {
            let mut text = group_lines(g, now, theme);
            if app.frame().expanded == Some(i) {
                text.extend(stack_lines(g, room.saturating_sub(text.len()), theme));
            }
            let height = text.len() as u16;
            Row::new([Cell::from(text)]).height(height)
        })
        .collect();
    let rows = match rows.is_empty() {
        true => vec![empty_row(0, " none".into(), theme)],
        false => rows,
    };
    let mut state =
        TableState::default().with_selected((!list.is_empty()).then_some(app.frame().cursor));
    let table = Table::new(rows, [Constraint::Fill(1)]).block(block(title.into(), theme));
    f.render_stateful_widget(table.row_highlight_style(theme.selected(true)), area, &mut state);
}

/// The group and its sample message. Stderr lines count no retries, so no "for good".
fn group_lines(g: &ExceptionGroup, now: Timestamp, theme: Theme) -> Vec<Line<'static>> {
    let for_good = match (g.component.as_str(), g.final_count) {
        ("_stderr", _) | (_, 0) => String::new(),
        (_, n) => format!(" ({n} for good)"),
    };
    let message = g.sample.as_ref().map_or("", |s| s.message.as_str());
    vec![
        Line::from(format!(
            "{}  {} {}  ×{}{for_good}  last {} ago",
            short(&Subject::Workload(g.workload.clone())),
            g.component,
            g.code,
            g.count,
            ago(now, g.last_seen)
        )),
        Line::styled(format!("  {message}"), theme.dim()),
    ]
}

/// The sample's stack in at most `room` lines, the last saying how many more there are.
fn stack_lines(g: &ExceptionGroup, room: usize, theme: Theme) -> Vec<Line<'static>> {
    let stack = g.sample.as_ref().and_then(|s| s.stacktrace.as_deref()).unwrap_or("(no stack)");
    let all: Vec<&str> = stack.lines().collect();
    let shown = if all.len() > room { room.saturating_sub(1) } else { all.len() };
    let mut lines: Vec<Line> =
        all[..shown].iter().map(|l| Line::styled(format!("    {l}"), theme.dim())).collect();
    if shown < all.len() && room > 0 {
        lines.push(Line::styled(format!("    … {} more lines", all.len() - shown), theme.dim()));
    }
    lines
}

fn state_lines(s: &WorkloadState, now: Timestamp, theme: Theme) -> Vec<Line<'static>> {
    let mb = |b: u64| format!("{} MB", b / 1_000_000);
    let since = s.state_since.map_or(String::new(), |t| format!(" for {}", ago(now, t)));
    let exit = s.exit_code.map_or(String::new(), |c| format!(" ({c})"));
    let oom = if s.oom_killed { ", OOM killed" } else { "" };
    // A stopped container still carries its last health: it says nothing now.
    let health = match (
        s.health.filter(|_| s.run == RunState::Running),
        s.health_failing_streak,
        &s.health_output,
    ) {
        (None, ..) => "—".to_string(),
        (Some(Health::Unhealthy), streak, output) => format!(
            "unhealthy{}{}",
            streak.map_or(String::new(), |n| format!(", {n} failed checks")),
            output.as_deref().map_or(String::new(), |o| format!(": {}", reason(o)))
        ),
        (Some(h), ..) => format!("{h:?}").to_lowercase(),
    };
    let mut lines = vec![
        Line::from(format!(" run       {}{exit}{since}{oom}", s.run)),
        Line::from(format!(" health    {health}")),
        Line::from(format!(" memory    {}", s.memory_used_bytes.map_or("—".into(), mb))),
        Line::from(format!(" restarts  {} in the last hour", s.restarts.len())),
    ];
    if let Some(d) = &s.datastore {
        let color = if d.reachable { Color::Green } else { Color::Red };
        lines.push(Line::styled(format!(" probe     {}", d.detail), theme.fg(color)));
    }
    lines
}

fn fact_lines(facts: Option<&WorkloadFacts>) -> Vec<Line<'static>> {
    let Some(f) = facts else { return vec![Line::from(" not known yet")] };
    let logs = match (&f.log_driver, &f.log_max_size) {
        (Some(d), Some(max)) => format!("{d}, max {max}"),
        (Some(d), None) => format!("{d}, no size limit"),
        (None, _) => "—".into(),
    };
    vec![
        Line::from(format!(" image     {}", f.image)),
        Line::from(format!(" restart   {}", f.restart_policy.as_deref().unwrap_or("no"))),
        Line::from(format!(
            " ports     {}",
            if f.ports.is_empty() { "—".into() } else { f.ports.join(", ") }
        )),
        Line::from(format!(
            " memory    {}",
            f.memory_limit_bytes
                .map_or("no limit".into(), |b| format!("limit {} MB", b / 1_000_000))
        )),
        Line::from(format!(" logs      {logs}")),
    ]
}
