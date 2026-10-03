//! Drawing. Every frame is drawn from the state alone; nothing here changes it.

mod host;
mod overview;
mod timeline;
mod workload;

use crate::api::FetchError;
use crate::app::{App, Screen};
use crate::problems::{self, Age};
use jiff::Timestamp;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table};
use skym_core::rules::Severity;
use skym_core::time::format_duration;
use skym_core::view::{IncidentView, Status};

/// Colors, unless `NO_COLOR` asks for none: then the symbols carry the meaning alone.
#[derive(Clone, Copy)]
pub struct Theme {
    pub color: bool,
}

impl Theme {
    fn fg(self, color: Color) -> Style {
        if self.color { Style::new().fg(color) } else { Style::new() }
    }

    fn status(self, s: Status) -> Span<'static> {
        let (mark, color) = match s {
            Status::Critical => ("✗", Color::Red),
            Status::Warn => ("!", Color::Yellow),
            Status::Unknown => ("?", Color::Magenta),
            Status::Ok => ("✓", Color::Green),
        };
        Span::styled(mark, self.fg(color))
    }

    fn severity(self, s: Severity) -> Span<'static> {
        let (mark, color) = match s {
            Severity::Critical => ("✗", Color::Red),
            Severity::Warn => ("!", Color::Yellow),
            Severity::Info => ("·", Color::DarkGray),
            Severity::Unknown => ("?", Color::Magenta),
        };
        Span::styled(mark, self.fg(color))
    }

    fn dim(self) -> Style {
        if self.color { Style::new().fg(Color::DarkGray) } else { Style::new() }
    }

    fn selected(self, focused: bool) -> Style {
        match (self.color, focused) {
            (true, true) => Style::new().bg(Color::DarkGray).add_modifier(Modifier::BOLD),
            (true, false) => Style::new().add_modifier(Modifier::BOLD),
            (false, _) => Style::new().add_modifier(Modifier::REVERSED),
        }
    }
}

pub fn draw(f: &mut Frame, app: &App, server: &str, now: Timestamp, theme: Theme) {
    let [top, body, bottom] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(0), Constraint::Length(1)])
            .areas(f.area());
    f.render_widget(top_bar(app, server, now, theme), top);
    match &app.frame().screen {
        Screen::Overview => overview::draw(f, body, app, now, theme),
        Screen::Host(_) => host::draw(f, body, app, now, theme),
        Screen::Workload(_) => workload::draw(f, body, app, now, theme),
        Screen::Exceptions(h) => workload::exceptions(f, body, app, h, now, theme),
        Screen::Timeline { .. } => timeline::draw(f, body, app, theme),
    }
    f.render_widget(bottom_bar(app, theme), bottom);
    if app.help {
        help(f, theme);
    }
}

fn top_bar(app: &App, server: &str, now: Timestamp, theme: Theme) -> Paragraph<'static> {
    let host = server.split("://").nth(1).unwrap_or(server).to_string();
    let mut spans = vec![
        Span::styled(" skym", Style::new().add_modifier(Modifier::BOLD)),
        Span::raw(format!(" · {host}   ")),
    ];
    if let Some(o) = &app.overview.value {
        let hosts: Vec<Status> =
            o.customers.iter().flat_map(|c| &c.hosts).map(|h| h.status).collect();
        for status in [Status::Critical, Status::Unknown, Status::Warn, Status::Ok] {
            let n = hosts.iter().filter(|s| **s == status).count();
            if n > 0 {
                let name = format!("{status:?}").to_lowercase();
                spans.extend([theme.status(status), Span::raw(format!(" {n} {name}   "))]);
            }
        }
    }
    let age = app.overview.at.map(|t| ago(now, t));
    match &app.error {
        Some(e) => {
            let why = match e {
                FetchError::Unreachable(why) => format!("cannot reach the server: {why}"),
                FetchError::Bad(why) => format!("unexpected answer: {why}"),
                FetchError::Unauthorized => "token rejected".into(),
            };
            let shown = age.map_or(String::new(), |a| format!(" · showing data from {a} ago"));
            spans.push(Span::styled(
                format!("{why}{shown}"),
                theme.fg(Color::Red).add_modifier(Modifier::BOLD),
            ));
        }
        None => spans.push(Span::styled(
            age.map_or("loading…".into(), |a| format!("updated {a} ago")),
            theme.dim(),
        )),
    }
    Paragraph::new(Line::from(spans))
}

fn bottom_bar(app: &App, theme: Theme) -> Paragraph<'static> {
    if app.editing || app.filter.is_some() {
        let text = app.filter.clone().unwrap_or_default();
        let cursor = if app.editing { "▏" } else { "" };
        return Paragraph::new(format!(" /{text}{cursor}   esc clear"));
    }
    let keys = match &app.frame().screen {
        Screen::Overview => {
            "↑↓ move  ⏎ open  ⇥ pane  / filter  h hygiene  m muted  r refresh  ? help  q quit"
        }
        Screen::Host(_) => {
            "↑↓ move  ⏎ service  t timeline  e exceptions  / filter  esc back  ? help"
        }
        Screen::Workload(_) => "↑↓ move  ⏎ expand exception  t timeline  esc back  ? help",
        Screen::Exceptions(_) => "↑↓ move  ⏎ expand  esc back  ? help",
        Screen::Timeline { .. } => "↑↓ move  [ ] window  esc back  ? help",
    };
    Paragraph::new(format!(" {keys}")).style(theme.dim())
}

fn help(f: &mut Frame, theme: Theme) {
    let lines = [
        "↑↓ j k     move",
        "⏎          open / expand",
        "esc ⌫      back (or clear the filter)",
        "⇥          switch pane (overview)",
        "/          filter by name",
        "t          timeline (host, service)",
        "e          exceptions (host)",
        "[ ]        timeline window: 6h 24h 7d",
        "h          show hygiene (info) problems",
        "m          show muted problems",
        "r          refresh now (every 30 s anyway)",
        "q          quit",
        "",
        "✗ critical   ! warn   ? unknown   ✓ ok   · info",
        "≥ age: the problem predates skym; at least this long",
    ];
    let area = centered(f.area(), 58, lines.len() as u16 + 2);
    f.render_widget(Clear, area);
    let block = Block::new()
        .borders(Borders::ALL)
        .title(" help · any key closes ")
        .border_style(theme.dim());
    f.render_widget(Paragraph::new(lines.join("\n")).block(block), area);
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    Rect::new(area.x + (area.width - w) / 2, area.y + (area.height - h) / 2, w, h)
}

/// A time as the clock on the wall shows it.
fn local(t: Timestamp, format: &str) -> String {
    t.to_zoned(jiff::tz::TimeZone::system()).strftime(format).to_string()
}

/// `5m`, `3d4h`: how long ago.
fn ago(now: Timestamp, then: Timestamp) -> String {
    format_duration(now.duration_since(then))
}

fn age(a: Age) -> String {
    match a {
        Age::Exact(d) => format_duration(d),
        Age::AtLeast(d) => format!("≥{}", format_duration(d)),
    }
}

/// An incident's detail without Docker's boilerplate, which would push the cause out of
/// a narrow column.
fn reason(detail: &str) -> String {
    detail.replace("OCI runtime exec failed: exec failed: unable to start container process: ", "")
}

fn event(kind: &skym_core::model::EventKind) -> String {
    use skym_core::model::EventKind;
    match kind {
        EventKind::Deployed { from, to } => format!("deployed {to} (was {from})"),
        EventKind::ConfigChanged => "configuration changed".into(),
        EventKind::Restarted => "restarted".into(),
        EventKind::OomKilled => "OOM killed".into(),
        EventKind::HostRebooted => "host rebooted".into(),
        EventKind::KernelChanged => "kernel changed".into(),
        EventKind::Unknown => "(an event this version does not know)".into(),
    }
}

/// Problems, worst first, each with how long it has lasted, and the height they take. As
/// on the overview: muted ones only on `m`, hygiene folded into one line unless `h`.
/// `name` fills the second column.
fn problem_table<'a>(
    app: &App,
    incidents: &'a [IncidentView],
    observed_since: Option<Timestamp>,
    now: Timestamp,
    name: impl Fn(&IncidentView) -> String,
    theme: Theme,
) -> (Table<'a>, u16) {
    let shown = |i: &&IncidentView| {
        (app.show_muted || !i.muted) && (app.show_info || i.severity != Severity::Info)
    };
    let mut listed: Vec<&IncidentView> = incidents.iter().filter(shown).collect();
    listed.sort_by(|a, b| b.severity.cmp(&a.severity).then(a.subject.cmp(&b.subject)));
    let folded =
        incidents.iter().filter(|i| !app.show_info && i.severity == Severity::Info).count();
    let mut rows: Vec<Row> = listed
        .into_iter()
        .map(|i| {
            Row::new([
                Cell::from(Line::from(vec![Span::raw(" "), theme.severity(i.severity)])),
                Cell::from(name(i)),
                Cell::from(reason(&i.detail)),
                Cell::from(Line::from(age(problems::age(i, observed_since, now))).right_aligned()),
            ])
        })
        .collect();
    if folded > 0 {
        rows.push(empty_row(2, format!("· {folded} hygiene items (h)"), theme));
    }
    if rows.is_empty() {
        rows.push(empty_row(1, "(none)".into(), theme));
    }
    let height = rows.len() as u16 + 2;
    let widths =
        [Constraint::Length(3), Constraint::Max(30), Constraint::Min(20), Constraint::Length(9)];
    (Table::new(rows, widths).block(block(" Problems ".into(), theme)), height)
}

/// A line of text in a table, in the given column: tables have no spanning cells.
fn empty_row(column: usize, text: String, theme: Theme) -> Row<'static> {
    let mut cells = vec![Cell::from(""); column];
    cells.push(Cell::from(text));
    Row::new(cells).style(theme.dim())
}

fn block(title: String, theme: Theme) -> Block<'static> {
    Block::new().borders(Borders::ALL).title(title).border_style(theme.dim())
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
