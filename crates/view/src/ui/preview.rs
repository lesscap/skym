//! The preview under (or beside) a list: what the selected row is, without opening it.

use super::Theme;
use super::format::{
    ago, answer, app_count, cores, cpu_use, local, memory, rate, reason, size, usage,
};
use crate::app::App;
use crate::apps::{self, Group, UNCLASSIFIED};
use crate::problems::{Age, Row};
use jiff::Timestamp;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use skym_core::view::{AppSummary, DiskUse, HostOverview, Status, WorkloadSummary};
use unicode_width::UnicodeWidthChar;

/// Where a preview line's text starts, past its label.
const INDENT: usize = 10;

/// A line too wide for `width` (in terminal cells: a CJK character takes two) goes on in
/// further lines under its text, broken before a ` · ` where it can be, else where the room
/// ends. Every part keeps its style.
pub fn wrap(line: Line<'static>, width: usize) -> Vec<Line<'static>> {
    if line.width() <= width || width <= INDENT + 10 {
        return vec![line];
    }
    let cells: Vec<(char, Style)> =
        line.spans.iter().flat_map(|s| s.content.chars().map(move |c| (c, s.style))).collect();
    let mut parts: Vec<&[(char, Style)]> = Vec::new();
    let mut rest = cells.as_slice();
    let mut room = width;
    loop {
        let mut used = 0;
        let fit = rest
            .iter()
            .take_while(|(c, _)| {
                used += c.width().unwrap_or(0);
                used <= room
            })
            .count();
        if fit == rest.len() {
            parts.push(rest);
            break;
        }
        // The first line keeps at least its label; a continuation may break anywhere.
        let floor = if parts.is_empty() { INDENT } else { 1 };
        let text: Vec<char> = rest[..fit].iter().map(|(c, _)| *c).collect();
        let separator = (floor..fit.saturating_sub(2))
            .rev()
            .find(|&i| text[i] == ' ' && text[i + 1] == '·' && text[i + 2] == ' ');
        let (at, skip) = separator.map_or((fit.max(1), 0), |i| (i, 3));
        parts.push(&rest[..at]);
        rest = &rest[at + skip..];
        room = width - INDENT;
    }
    parts
        .into_iter()
        .enumerate()
        .map(|(n, part)| {
            let indent = (n > 0).then(|| Span::raw(" ".repeat(INDENT)));
            Line::from(indent.into_iter().chain(styled(part)).collect::<Vec<_>>())
        })
        .collect()
}

/// Runs of one style as spans.
fn styled(cells: &[(char, Style)]) -> Vec<Span<'static>> {
    cells
        .chunk_by(|a, b| a.1 == b.1)
        .map(|run| Span::styled(run.iter().map(|(c, _)| *c).collect::<String>(), run[0].1))
        .collect()
}

pub fn label(text: &str, theme: Theme) -> Span<'static> {
    Span::styled(format!(" {text:<9}"), theme.dim())
}

/// `top mem  docker 2.3G · i 1.6G · baton 1.5G · 25 others 3.6G`: the three largest users of
/// one resource on a host, then the rest; none when no application reports it.
fn top(
    name: &str,
    apps: &[&AppSummary],
    used: impl Fn(&AppSummary) -> Option<f64>,
    show: impl Fn(f64) -> String,
    theme: Theme,
) -> Option<Line<'static>> {
    let mut using: Vec<(&str, f64)> =
        apps.iter().filter_map(|a| Some((a.name.as_str(), used(a)?))).collect();
    using.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(b.0)));
    let rest = using.split_off(using.len().min(3));
    let mut parts: Vec<String> =
        using.iter().map(|(name, value)| format!("{name} {}", show(*value))).collect();
    match rest.as_slice() {
        [] => {}
        [(name, value)] => parts.push(format!("{name} {}", show(*value))),
        _ => {
            let others: f64 = rest.iter().map(|(_, v)| v).sum();
            parts.push(format!("{} others {}", rest.len(), show(others)));
        }
    }
    (!parts.is_empty()).then(|| Line::from(vec![label(name, theme), Span::raw(parts.join(" · "))]))
}

/// `tags     acme · billing`, when it has any.
fn tags(tags: &[String], theme: Theme) -> Option<Line<'static>> {
    (!tags.is_empty()).then(|| Line::from(vec![label("tags", theme), Span::raw(tags.join(" · "))]))
}

/// A group of the applications tab: everything in it, those in trouble first.
pub fn group(g: &Group, theme: Theme) -> Vec<Line<'static>> {
    let trouble = g.all.iter().filter(|a| a.status != Status::Ok).count();
    let state = if g.open { "open" } else { "folded to its trouble (⏎ opens)" };
    let mut names = vec![label("apps", theme)];
    for (i, a) in g.all.iter().enumerate() {
        if i > 0 {
            names.push(Span::raw(" · "));
        }
        if a.status != Status::Ok {
            names.extend([theme.status(a.status), Span::raw(" ")]);
        }
        names.push(Span::raw(a.name.clone()));
    }
    vec![
        Line::from(format!(
            " {}{} · {trouble} in trouble · {state}",
            app_count(g.all.len()),
            g.memory().map_or(String::new(), |m| format!(" · {}", size(m)))
        )),
        Line::from(names),
    ]
}

/// One problem: all of it, its application, its workload and its host.
pub fn problem(app: &App, r: &Row, now: Timestamp, theme: Theme) -> Vec<Line<'static>> {
    let i = r.incident;
    let mut lines = vec![Line::from(vec![
        Span::raw(" "),
        theme.severity(i.severity),
        Span::raw(format!(" {}  {}", i.code.as_str(), i.detail)),
    ])];
    let opened =
        i.opened_at.map_or(String::new(), |t| format!("opened {}", local(t, "%m-%d %H:%M")));
    let watched = match (r.age, i.observed_since) {
        (Age::AtLeast(_), Some(t)) => {
            format!(" · skym watches it since {}: it is older", local(t, "%m-%d %H:%M"))
        }
        _ => String::new(),
    };
    lines.push(Line::from(vec![label("since", theme), Span::raw(format!("{opened}{watched}"))]));
    match r.app().and_then(|k| app.app_summary(k)) {
        Some(a) => lines.push(Line::from(vec![label("app", theme), Span::raw(app_line(a))])),
        None if r.app().is_none() => lines.push(Line::from(vec![
            label("app", theme),
            Span::styled("— (the host's own)", theme.dim()),
        ])),
        None => {}
    }
    if let Some(w) = &i.workload {
        lines.push(Line::from(vec![label("service", theme), Span::raw(workload_line(w, now))]));
        if let Some(out) = &w.health_output {
            lines.push(Line::from(vec![label("health", theme), Span::raw(reason(out))]));
        }
    }
    if let Some(h) = app.host_summary(r.host) {
        lines.push(Line::from(vec![label("host", theme), Span::raw(host_line(h, now))]));
    }
    lines
}

fn app_line(a: &AppSummary) -> String {
    let env = a.env.as_deref().unwrap_or(UNCLASSIFIED);
    let note = a.note.as_deref().map_or(String::new(), |n| format!(" · {n}"));
    format!("{} · {env}{note} · {}/{} running", a.name, a.running, a.services)
}

/// `api · registry/shop:1.4 · running for 3d · restart always · 0.0.0.0:8080->80/tcp · 120M / 512M`
fn workload_line(w: &WorkloadSummary, now: Timestamp) -> String {
    let mut parts = vec![w.key.service.clone()];
    parts.extend(w.image.clone());
    let since = w.state_since.map_or(String::new(), |t| format!(" for {}", ago(now, t)));
    parts.push(format!("{}{since}", w.run));
    parts.extend(w.restart_policy.as_ref().map(|p| format!("restart {p}")));
    if !w.ports.is_empty() {
        parts.push(w.ports.join(", "));
    }
    parts.extend(w.memory_used_bytes.map(|used| usage(used, w.memory_limit_bytes)));
    if w.restarts_last_hour > 0 {
        parts.push(format!("{} restarts in the last hour", w.restarts_last_hour));
    }
    parts.join(" · ")
}

fn host_line(h: &HostOverview, now: Timestamp) -> String {
    let mut parts = vec![h.id.clone()];
    parts.extend(h.os.clone());
    parts.extend(h.ip.clone());
    parts.extend(h.boot_time.map(|t| format!("up {}", ago(now, t))));
    parts.extend(h.cpu_percent.map(|p| format!("cpu {p:.0}%")));
    parts.extend(h.load_1m.map(|l| format!("load {l:.2}")));
    parts.push(format!("mem {}", memory(h.memory_used_bytes, h.memory_total_bytes)));
    let disks: Vec<String> =
        h.disks.iter().map(|d| format!("{} {}%", d.path, d.used_percent)).collect();
    if !disks.is_empty() {
        parts.push(disks.join("  "));
    }
    parts.join(" · ")
}

/// One application: what it is, its URLs, its workloads, its latest deployments.
pub fn application(a: &AppSummary, now: Timestamp, theme: Theme) -> Vec<Line<'static>> {
    let workloads = a.workloads.iter().map(|w| {
        Line::from(vec![
            label("service", theme),
            theme.status(w.status),
            Span::raw(format!(" {}", workload_line(w, now))),
        ])
    });
    let mut lines = app_head(a, theme);
    lines.extend(workloads);
    lines.extend(app_history(a, 3, now, theme));
    lines
}

/// What an application is, and its URLs.
pub fn app_head(a: &AppSummary, theme: Theme) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(vec![
        Span::raw(" "),
        theme.status(a.status),
        Span::raw(format!(" {}  {}  {}", a.name, a.key, a.env.as_deref().unwrap_or(UNCLASSIFIED))),
    ])];
    if let Some(note) = &a.note {
        lines.push(Line::from(vec![label("note", theme), Span::raw(note.clone())]));
    }
    lines.extend(tags(&a.tags, theme));
    for e in &a.endpoints {
        let answer = answer(e, true);
        let cert = e
            .cert_expires_at
            .map_or(String::new(), |t| format!(" · cert until {}", local(t, "%Y-%m-%d")));
        lines.push(Line::from(vec![
            label("url", theme),
            theme.status(e.status),
            Span::raw(format!(" {}  {answer}{cert}", e.url)),
        ]));
    }
    lines
}

/// Its latest `deploys` deployments and its errors of the last hour.
pub fn app_history(
    a: &AppSummary,
    deploys: usize,
    now: Timestamp,
    theme: Theme,
) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = a
        .deploys
        .iter()
        .take(deploys)
        .map(|d| {
            let line = format!("{} ago  {}  {} → {}", ago(now, d.ts), d.service, d.from, d.to);
            Line::from(vec![label("deployed", theme), Span::raw(line)])
        })
        .collect();
    if a.exceptions_1h > 0 {
        let line = format!("{} application exceptions in the last hour", a.exceptions_1h);
        lines.push(Line::from(vec![
            label("errors", theme),
            Span::styled(line, theme.fg(Color::Yellow)),
        ]));
    }
    lines
}

/// One host: who it is, then [`host_body`].
pub fn host(app: &App, h: &HostOverview, now: Timestamp, theme: Theme) -> Vec<Line<'static>> {
    let title = Line::from(vec![
        Span::raw(" "),
        theme.status(h.status),
        Span::raw(format!(" {}  {}", h.id, h.ip.as_deref().unwrap_or("address unknown"))),
    ]);
    std::iter::once(title).chain(host_body(app, h, now, theme)).collect()
}

/// What a host runs on, its load and memory, its disks in full, its applications in trouble.
pub fn host_body(app: &App, h: &HostOverview, now: Timestamp, theme: Theme) -> Vec<Line<'static>> {
    let system: Vec<String> = [
        h.os.clone(),
        h.kernel.as_ref().map(|k| format!("kernel {k}")),
        h.arch.clone(),
        h.cpu_count.map(|n| format!("{n} cpu")),
        h.boot_time.map(|t| format!("up {}", ago(now, t))),
        h.docker_version.as_ref().map(|v| format!("docker {v}")),
        h.agent_version.as_ref().map(|v| format!("skym {v}")),
    ]
    .into_iter()
    .flatten()
    .collect();
    let mut lines = vec![
        Line::from(vec![label("system", theme), Span::raw(system.join(" · "))]),
        Line::from(vec![
            label("cpu", theme),
            Span::raw(format!(
                "{}   memory {}",
                cpu_use(h),
                memory(h.memory_used_bytes, h.memory_total_bytes)
            )),
        ]),
    ];
    if let (Some(rx), Some(tx)) = (h.net_rx_bytes_per_s, h.net_tx_bytes_per_s) {
        lines.push(Line::from(vec![
            label("net", theme),
            Span::raw(format!("↓ {}  ↑ {}", rate(rx), rate(tx))),
        ]));
    }
    let on_host: Vec<&AppSummary> = app.apps_iter().filter(|a| a.key.host == h.id).collect();
    lines.extend(tags(&h.tags, theme));
    lines.extend(h.disks.iter().map(|d| disk_line(d, theme)));
    let troubled: Vec<String> =
        on_host.iter().filter(|a| a.status != Status::Ok).map(|a| a.name.clone()).collect();
    if !troubled.is_empty() {
        lines.push(Line::from(vec![label("trouble", theme), Span::raw(troubled.join(", "))]));
    }
    let memory = |a: &AppSummary| apps::used_memory(a).map(|b| b as f64);
    lines.extend(top("top mem", &on_host, memory, |b| size(b as u64), theme));
    let cpu = |a: &AppSummary| apps::used_cpu(a).map(f64::from);
    lines.extend(top("top cpu", &on_host, cpu, |c| cores(c as f32), theme));
    lines
}

/// `/data  xfs  80G  35G free  44%  inodes 12%  ▲ filling up`
pub fn disk_line(d: &DiskUse, theme: Theme) -> Line<'static> {
    let fs = d.fs_type.clone().unwrap_or_default();
    let text = format!(
        "{:<12} {:<6} {:>6}  {:>6} free  {:>3}%  inodes {}%",
        d.path,
        fs,
        size(d.total_bytes),
        size(d.free_bytes),
        d.used_percent,
        d.inodes_percent
    );
    let warn = d.filling || d.used_percent >= 85;
    let style = if warn { theme.fg(Color::Yellow) } else { Style::new() };
    let mut spans = vec![label("disk", theme), Span::styled(text, style)];
    if d.filling {
        spans.push(Span::styled("  ▲ filling up", theme.fg(Color::Yellow)));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_lines_go_on_under_their_text() {
        let text = "api · shop:1.4 · running for 3d · restart always";
        let line = Line::from(vec![Span::raw(" service  "), Span::raw(text)]);
        let wrapped: Vec<String> = wrap(line, 40).iter().map(ToString::to_string).collect();
        assert_eq!(
            wrapped,
            [" service  api · shop:1.4", "          running for 3d", "          restart always"]
        );
        let short = Line::from(" disk     / 35%");
        assert_eq!(wrap(short.clone(), 40), [short]);
        let unbroken = Line::from(format!(" health   {}", "x".repeat(50)));
        let parts: Vec<String> = wrap(unbroken, 40).iter().map(ToString::to_string).collect();
        assert_eq!(parts.len(), 2);
        assert!(
            parts[1].starts_with("          x") && parts.iter().all(|p| p.chars().count() <= 40)
        );
    }

    #[test]
    fn wide_characters_take_two_cells_and_styles_survive_a_break() {
        let dim = Style::new().add_modifier(ratatui::style::Modifier::DIM);
        let line = Line::from(vec![
            Span::styled(" note     ", dim),
            Span::raw("学习平台后台 · 服务两个 · 每天部署"),
        ]);
        let wrapped = wrap(line, 30);
        assert!(wrapped.iter().all(|l| l.width() <= 30), "{wrapped:?}");
        assert_eq!(wrapped[0].spans[0].style, dim, "the label keeps its style");
        // 10 + 12 cells fit, `· 服务两个` would make 33: break before the separator.
        assert_eq!(wrapped[0].to_string(), " note     学习平台后台");
        assert_eq!(wrapped[1].to_string(), "          服务两个 · 每天部署");
    }
}
