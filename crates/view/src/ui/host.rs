//! One host: what it is, how it is doing, its problems and its services.

use super::apps::host_apps;
use super::{Theme, ago, local, problem_table};
use crate::app::App;
use crate::names::short;
use jiff::Timestamp;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use skym_core::rules::IncidentCode;
use skym_core::subject::Subject;
use skym_core::view::{HostView, IncidentView};

pub fn draw(f: &mut Frame, area: Rect, app: &App, now: Timestamp, theme: Theme) {
    let Some(h) = &app.host.value else {
        return f.render_widget(Paragraph::new(" loading…"), area);
    };
    let head = header(h, now, theme);
    // The host's own problems: its applications' are on them.
    let own: Vec<IncidentView> = h
        .incidents
        .iter()
        .filter(|i| matches!(i.subject, Subject::Host(_) | Subject::Mount { .. }))
        .cloned()
        .collect();
    let (problems, height) = problem_table(app, &own, now, |i| short(&i.subject), theme);
    let [head_area, problems_area, apps] = Layout::vertical([
        Constraint::Length(head.len() as u16),
        Constraint::Length(height.min((area.height / 3).max(3))),
        Constraint::Min(3),
    ])
    .areas(area);
    f.render_widget(Paragraph::new(head), head_area);
    f.render_widget(problems, problems_area);
    host_apps(f, apps, app, now, theme);
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
            let filling = h.incidents.iter().any(|i| {
                !i.muted
                    && i.code == IncidentCode::DiskFilling
                    && matches!(&i.subject, Subject::Mount { path, .. } if *path == m.path)
            });
            let style = if used >= 85 || filling { theme.fg(Color::Yellow) } else { Style::new() };
            let mark = if filling { " ▲" } else { "" };
            spans.push(Span::styled(format!("  {} {used}%{mark}", m.path), style));
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
