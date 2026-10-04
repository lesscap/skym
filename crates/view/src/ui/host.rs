//! One host: what it is, how it is doing, its problems and its services.

use super::apps::host_apps;
use super::format::local;
use super::{Theme, hygiene_folded, preview, problem_table, shown};
use crate::app::App;
use crate::names::short;
use jiff::Timestamp;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Color;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use skym_core::subject::Subject;
use skym_core::view::{HostView, IncidentView};

pub fn draw(f: &mut Frame, area: Rect, app: &App, now: Timestamp, theme: Theme) {
    let Some(h) = &app.host.value else {
        return f.render_widget(Paragraph::new(" loading…"), area);
    };
    let mut head = header(app, h, now, theme);
    // The host's own problems: its applications' are on them. With none to list, a line.
    let own: Vec<IncidentView> = h
        .incidents
        .iter()
        .filter(|i| matches!(i.subject, Subject::Host(_) | Subject::Mount { .. }))
        .cloned()
        .collect();
    let (problems, height) = problem_table(app, &own, now, |i| short(&i.subject), theme);
    let listed = own.iter().any(|i| shown(app, i));
    if !listed {
        let hygiene = match hygiene_folded(app, &own) {
            0 => String::new(),
            n => format!(" · {n} hygiene items (h)"),
        };
        head.push(Line::from(vec![
            preview::label("problems", theme),
            Span::raw(format!("none{hygiene}")),
        ]));
    }
    let height = if listed { height.min((area.height / 3).max(3)) } else { 0 };
    let [head_area, problems_area, apps] = Layout::vertical([
        Constraint::Length(head.len() as u16),
        Constraint::Length(height),
        Constraint::Min(3),
    ])
    .areas(area);
    f.render_widget(Paragraph::new(head), head_area);
    if listed {
        f.render_widget(problems, problems_area);
    }
    host_apps(f, apps, app, now, theme);
}

/// Who it is and since when skym knows it, then what the hosts tab previews: system,
/// load and memory, every disk in full, its applications in trouble. Then any collection
/// errors.
fn header(app: &App, h: &HostView, now: Timestamp, theme: Theme) -> Vec<Line<'static>> {
    let watched = h
        .observed_since
        .map_or(String::new(), |t| format!("   watched since {}", local(t, "%m-%d %H:%M")));
    let reported =
        h.last_report_ago.clone().map_or("never reported".into(), |a| format!("report {a} ago"));
    let ip = h.ip.as_deref().map_or(String::new(), |ip| format!("   {ip}"));
    let mut lines = vec![Line::from(vec![
        Span::raw(format!(" {}  ", h.id)),
        theme.status(h.status),
        Span::raw(format!(" {:?}   {reported}{watched}{ip}", h.status).to_lowercase()),
    ])];
    let summary = app.host_summary(&h.id);
    if let Some(s) = summary {
        lines.extend(preview::host_body(app, s, now, theme));
    }
    if !h.errors.is_empty() {
        lines.push(Line::styled(
            format!(" collection errors: {}", h.errors.join("; ")),
            theme.fg(Color::Yellow),
        ));
    }
    lines
}
