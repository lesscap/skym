//! One service: its problems, state and facts side by side, exceptions, recent events.
//! Also a host's exceptions, which read the same way.

use super::{Theme, age, ago, block, event, reason, short};
use crate::app::App;
use crate::problems;
use jiff::Timestamp;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Color;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table, TableState};
use skym_core::model::{ExceptionGroup, Health, RunState, WorkloadFacts, WorkloadState};

pub fn draw(f: &mut Frame, area: Rect, app: &App, now: Timestamp, theme: Theme) {
    let Some(w) = &app.workload.value else {
        return f.render_widget(Paragraph::new(" loading…"), area);
    };
    let observed = app.observed_since(&w.key.host);
    let problems_height = (w.incidents.len() as u16 + 2).clamp(3, 8);
    let [head, problems_area, details, exceptions_area, events_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(problems_height),
        Constraint::Length(8),
        Constraint::Min(4),
        Constraint::Length(8),
    ])
    .areas(area);
    let name = format!("{}/{}", w.key.project, w.key.service);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(format!(" {} › {name}  ", w.key.host)),
            theme.status(w.status),
            Span::raw(format!(" {:?}", w.status).to_lowercase()),
        ])),
        head,
    );
    let rows: Vec<Row> = w
        .incidents
        .iter()
        .map(|i| {
            Row::new([
                Cell::from(Line::from(vec![Span::raw(" "), theme.severity(i.severity)])),
                Cell::from(i.code.as_str()),
                Cell::from(reason(&i.detail)),
                Cell::from(Line::from(age(problems::age(i, observed, now))).right_aligned()),
            ])
        })
        .collect();
    let widths =
        [Constraint::Length(3), Constraint::Length(22), Constraint::Fill(1), Constraint::Length(9)];
    let rows = match rows.is_empty() {
        true => vec![Row::new([Cell::from(""), Cell::from("(none)")]).style(theme.dim())],
        false => rows,
    };
    f.render_widget(
        Table::new(rows, widths).block(block(" Problems ".into(), theme)),
        problems_area,
    );
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)]).areas(details);
    f.render_widget(
        Paragraph::new(state_lines(&w.state, now, theme)).block(block(" State ".into(), theme)),
        left,
    );
    f.render_widget(
        Paragraph::new(fact_lines(w.facts.as_ref())).block(block(" Facts ".into(), theme)),
        right,
    );
    groups(f, exceptions_area, app, &w.exceptions, " Exceptions · last hour ", now, theme);
    let events: Vec<Line> = w
        .events
        .iter()
        .rev()
        .map(|e| Line::from(format!(" {:>6} ago  {}", ago(now, e.ts), event(&e.kind))))
        .collect();
    let events = if events.is_empty() {
        vec![Line::styled(" no events in the last day", theme.dim())]
    } else {
        events
    };
    f.render_widget(
        Paragraph::new(events).block(block(" Events · 24h ".into(), theme)),
        events_area,
    );
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
    let cursor = app.frame().cursor;
    let rows: Vec<Row> = list
        .iter()
        .enumerate()
        .map(|(i, g)| {
            let sample = g.sample.as_ref();
            let message = sample.map_or("", |s| s.message.as_str());
            let mut text = vec![
                Line::from(format!(
                    "{}  {} {}  ×{}{}  last {} ago",
                    short(&skym_core::subject::Subject::Workload(g.workload.clone())),
                    g.component,
                    g.code,
                    g.count,
                    if g.final_count > 0 {
                        format!(" ({} for good)", g.final_count)
                    } else {
                        String::new()
                    },
                    ago(now, g.last_seen)
                )),
                Line::styled(format!("  {message}"), theme.dim()),
            ];
            if app.frame().expanded == Some(i) {
                let stack = sample.and_then(|s| s.stacktrace.as_deref()).unwrap_or("(no stack)");
                text.extend(stack.lines().map(|l| Line::styled(format!("    {l}"), theme.dim())));
            }
            let height = text.len() as u16;
            Row::new([Cell::from(text)]).height(height)
        })
        .collect();
    let rows = if rows.is_empty() {
        vec![Row::new([Cell::from(Span::styled(" none", theme.dim()))])]
    } else {
        rows
    };
    let mut state = TableState::default().with_selected((!list.is_empty()).then_some(cursor));
    let table = Table::new(rows, [Constraint::Fill(1)])
        .block(block(title.into(), theme))
        .row_highlight_style(theme.selected(true));
    f.render_stateful_widget(table, area, &mut state);
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
