//! Drawing. Every frame is drawn from the state alone; nothing here changes it.

mod apps;
mod format;
mod host;
mod hosts;
mod preview;
mod problems;
mod timeline;
mod workload;

use crate::api::FetchError;
use crate::app::{App, Screen, TABS};
use crate::apps::Sort;
use format::{age, ago, reason};
use jiff::Timestamp;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table};
use skym_core::rules::Severity;
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
        Screen::Problems => problems::draw(f, body, app, now, theme),
        Screen::Hosts => hosts::draw(f, body, app, now, theme),
        Screen::Host(_) => host::draw(f, body, app, now, theme),
        Screen::Workload(_) => workload::draw(f, body, app, now, theme),
        Screen::Exceptions(h) => workload::exceptions(f, body, app, h, now, theme),
        Screen::Timeline { .. } => timeline::draw(f, body, app, theme),
        Screen::Apps => apps::list(f, body, app, now, theme),
        Screen::App(_) => apps::one(f, body, app, now, theme),
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
    for tab in TABS {
        let name = match tab {
            Screen::Problems => "Problems",
            Screen::Apps => "Apps",
            _ => "Hosts",
        };
        spans.push(match *app.tab() == tab {
            true => Span::styled(format!("[{name}] "), Style::new().add_modifier(Modifier::BOLD)),
            false => Span::styled(format!(" {name}  "), theme.dim()),
        });
    }
    spans.push(Span::raw("    "));
    // A server older than `problems` and `hosts` sends neither, yet a status they would
    // explain: it would look like a calm fleet, so say so.
    let older = app
        .overview
        .value
        .as_ref()
        .is_some_and(|o| o.hosts.is_empty() && o.problems.is_empty() && o.status != Status::Ok);
    if older {
        spans.push(Span::styled(
            "the server is older than this view: upgrade it   ",
            theme.fg(Color::Red).add_modifier(Modifier::BOLD),
        ));
    } else if let Some(o) = &app.overview.value {
        // Open problems by severity, and hosts nobody has heard from.
        let count = |s: Severity| o.problems.iter().filter(|i| i.severity == s).count();
        let unknown = o.hosts.iter().filter(|h| h.status == Status::Unknown).count();
        for (n, mark) in [
            (count(Severity::Critical), theme.status(Status::Critical)),
            (count(Severity::Warn), theme.status(Status::Warn)),
            (unknown, theme.status(Status::Unknown)),
        ] {
            if n > 0 {
                spans.extend([mark, Span::raw(format!(" {n}   "))]);
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
        Screen::Problems => {
            "↑↓ move  ⏎ open  ⇥ apps  / filter  h hygiene  m muted  p preview  ? help  q quit"
        }
        Screen::Apps => {
            "↑↓ move  ⏎ open/fold  ⇥ hosts  g group  e all  s sort  / filter  ? help  q quit"
        }
        Screen::Hosts => "↑↓ move  ⏎ open  ⇥ problems  / filter  p preview  ? help  q quit",
        Screen::App(_) => "↑↓ move  ⏎ service  ⇥ next tab  / filter  esc back  ? help",
        Screen::Host(_) => "↑↓ move  ⏎ app  s sort  t timeline  e exceptions  esc back  ? help",
        Screen::Workload(_) => "↑↓ move  ⏎ expand exception  t timeline  esc back  ? help",
        Screen::Exceptions(_) => "↑↓ move  ⏎ expand  esc back  ? help",
        Screen::Timeline { .. } => "↑↓ move  [ ] window  esc back  ? help",
    };
    Paragraph::new(format!(" {keys}")).style(theme.dim())
}

fn help(f: &mut Frame, theme: Theme) {
    let labels = Sort::ALL.map(Sort::label);
    let sorts = match labels.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{}, or {last}", rest.join(", ")),
        _ => labels.join(""),
    };
    let sorts = format!("s          sort applications: {sorts}");
    let lines = [
        "↑↓ j k     move",
        "⏎          open / expand (a group of applications: open or fold)",
        "esc ⌫      back (or clear the filter)",
        "⇥          next tab: problems, apps, hosts",
        "/          filter: words, host:<id> env:<env> tag:<tag> !ok",
        "t          timeline (host, service)",
        "e          exceptions (host); open or fold every group (applications)",
        sorts.as_str(),
        "g          group applications by environment, host or tag",
        "[ ]        timeline window: 6h 24h 7d",
        "h          show hygiene (info) problems",
        "p          preview of the selected row (tabs)",
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

/// A tab's list and, when shown, its preview: beside the list from 160 columns, under it
/// (a third of the height, at least 7 rows) below that.
fn with_preview(area: Rect, shown: bool) -> (Rect, Option<Rect>) {
    if !shown {
        return (area, None);
    }
    if area.width >= 160 {
        let [list, preview] =
            Layout::horizontal([Constraint::Fill(3), Constraint::Fill(2)]).areas(area);
        return (list, Some(preview));
    }
    let [list, preview] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length((area.height / 3).max(7))])
            .areas(area);
    (list, Some(preview))
}

/// The preview of the selected row in a box named after it; long lines go on under their
/// text, past the labels.
fn draw_preview(f: &mut Frame, area: Rect, title: String, lines: Vec<Line<'static>>, theme: Theme) {
    let width = area.width.saturating_sub(2) as usize;
    let lines: Vec<Line> = lines.into_iter().flat_map(|l| preview::wrap(l, width)).collect();
    f.render_widget(Paragraph::new(lines).block(block(title, theme)), area);
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    Rect::new(area.x + (area.width - w) / 2, area.y + (area.height - h) / 2, w, h)
}

/// Listed in a problems table: muted and hygiene ones only once asked for.
fn shown(app: &App, i: &IncidentView) -> bool {
    (app.show_muted || !i.muted) && (app.show_info || i.severity != Severity::Info)
}

/// Hygiene items left out until `h`.
fn hygiene_folded(app: &App, incidents: &[IncidentView]) -> usize {
    incidents.iter().filter(|i| !app.show_info && i.severity == Severity::Info).count()
}

/// Problems, worst first, each with how long it has lasted, and the height they take. As
/// on the overview: muted ones only on `m`, hygiene folded into one line unless `h`.
/// `name` fills the second column.
fn problem_table<'a>(
    app: &App,
    incidents: &'a [IncidentView],
    now: Timestamp,
    name: impl Fn(&IncidentView) -> String,
    theme: Theme,
) -> (Table<'a>, u16) {
    let mut listed: Vec<&IncidentView> = incidents.iter().filter(|i| shown(app, i)).collect();
    listed.sort_by(|a, b| b.severity.cmp(&a.severity).then(a.subject.cmp(&b.subject)));
    let folded = hygiene_folded(app, incidents);
    let mut rows: Vec<Row> = listed
        .into_iter()
        .map(|i| {
            Row::new([
                Cell::from(Line::from(vec![Span::raw(" "), theme.severity(i.severity)])),
                Cell::from(name(i)),
                Cell::from(reason(&i.detail)),
                Cell::from(
                    Line::from(age(crate::problems::age(i, i.observed_since, now))).right_aligned(),
                ),
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
