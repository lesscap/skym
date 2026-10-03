use super::*;
use crate::api::{Payload, Request};
use crate::app::{Msg, update};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use skym_core::rules::IncidentCode;
use skym_core::view::{CustomerOverview, EndpointOverview, HostOverview, IncidentView, Overview};
use std::collections::BTreeMap;

fn t(min: i64) -> Timestamp {
    Timestamp::from_second(1_790_000_000 + min * 60).unwrap()
}

fn incident(subject: &str, code: IncidentCode, severity: Severity, opened: i64) -> IncidentView {
    IncidentView {
        subject: subject.parse().unwrap(),
        code,
        severity,
        detail: format!("{} detail", code.as_str()),
        opened_at: Some(t(opened)),
        since: None,
        open_for: None,
        resolved_at: None,
        muted: false,
        mute_reason: None,
        links: BTreeMap::new(),
    }
}

/// The overview screen as text, 100 columns wide.
fn screen(theme: Theme) -> Vec<String> {
    overview_screen(theme, 100)
}

fn overview_screen(theme: Theme, width: u16) -> Vec<String> {
    let host = HostOverview {
        id: "x".into(),
        status: Status::Critical,
        last_report_ago: Some("5s".into()),
        observed_since: Some(t(0)),
        info_count: 1,
        incidents: vec![
            incident("workload:x/app/old", IncidentCode::WorkloadUnhealthy, Severity::Critical, 1),
            incident("workload:x/app/fresh", IncidentCode::WorkloadDown, Severity::Critical, 100),
            incident("host:x", IncidentCode::LogUnbounded, Severity::Info, 1),
        ],
        links: BTreeMap::new(),
    };
    let overview = Overview {
        ts: t(0),
        status: Status::Critical,
        customers: vec![CustomerOverview {
            id: "acme".into(),
            name: "Acme".into(),
            status: Status::Critical,
            hosts: vec![host],
            endpoints: vec![
                endpoint(
                    "https://shop.example.com/",
                    None,
                    vec![incident(
                        "endpoint:https://shop.example.com/",
                        IncidentCode::EndpointDown,
                        Severity::Critical,
                        110,
                    )],
                ),
                endpoint("https://api.example.com/healthz", Some(1234), vec![]),
            ],
        }],
        muted_count: 0,
    };
    let mut app = App::default();
    update(
        &mut app,
        Msg::Fetched(Request::Overview, Ok(Box::new(Payload::Overview(overview)))),
        t(120),
    );
    render(&app, theme, width, 16)
}

fn endpoint(url: &str, latency_ms: Option<u64>, incidents: Vec<IncidentView>) -> EndpointOverview {
    EndpointOverview {
        url: url.into(),
        status: if incidents.is_empty() { Status::Ok } else { Status::Critical },
        last_probe_ago: Some("20s".into()),
        observed_since: Some(t(0)),
        http_status: latency_ms.map(|_| 200),
        latency_ms,
        cert_expires_at: None,
        incidents,
    }
}

fn render(app: &App, theme: Theme, width: u16, height: u16) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|f| draw(f, app, "https://skym.example.com", t(120), theme)).unwrap();
    let buffer = terminal.backend().buffer();
    (0..buffer.area.height)
        .map(|y| (0..buffer.area.width).map(|x| buffer[(x, y)].symbol()).collect::<String>())
        .collect()
}

fn line_of(lines: &[String], text: &str) -> usize {
    lines
        .iter()
        .position(|l| l.contains(text))
        .unwrap_or_else(|| panic!("{text:?} in\n{}", lines.join("\n")))
}

#[test]
fn new_problems_lead_and_hygiene_stays_folded() {
    let lines = screen(Theme { color: true });
    assert!(line_of(&lines, "NEW") < line_of(&lines, " fresh "));
    assert!(line_of(&lines, " fresh ") < line_of(&lines, "ONGOING"));
    assert!(line_of(&lines, "ONGOING") < line_of(&lines, " old "));
    assert!(lines[line_of(&lines, " old ")].contains("≥2h"), "found at the first look");
    line_of(&lines, "1 hygiene items");
    assert!(!lines.iter().any(|l| l.contains("LOG_UNBOUNDED detail")), "folded");
}

#[test]
fn without_color_the_symbols_still_tell_the_status() {
    let lines = screen(Theme { color: false });
    assert!(lines[line_of(&lines, " fresh ")].contains('✗'));
    assert!(lines[line_of(&lines, " x ")].contains('✗'), "the host's status");
    line_of(&lines, "2 critical");
}

#[test]
fn on_a_narrow_terminal_the_reason_still_shows() {
    let lines = overview_screen(Theme { color: true }, 90);
    let row = &lines[line_of(&lines, " fresh ")];
    assert!(row.contains("WORKLOAD_DOWN detail"), "the whole reason fits: {row}");
}

/// A service screen with one exception group, its stack 40 lines long.
fn service_screen(component: &str, expanded: bool) -> Vec<String> {
    use crate::app::{Frame, Screen};
    let key = skym_core::subject::WorkloadKey {
        host: "x".into(),
        project: "app".into(),
        service: "api".into(),
    };
    let stack: Vec<String> = (1..=40).map(|i| format!("at frame{i}")).collect();
    let view: skym_core::view::WorkloadView = serde_json::from_value(serde_json::json!({
        "key": key, "status": "warn", "facts": null,
        "state": { "run": "running", "exit_code": null, "health": null, "memory_used_bytes": null, "datastore": null },
        "exceptions": [{ "workload": key, "class": "application", "component": component, "code": "E",
            "count": 180, "final_count": 180, "first_seen": "2026-10-01T00:00:00Z", "last_seen": "2026-10-01T00:00:00Z",
            "sample": { "message": "job is already running", "stacktrace": stack.join("\n") } }]
    }))
    .unwrap();
    let mut app = App::default();
    app.stack.push(Frame {
        screen: Screen::Workload(key.clone()),
        cursor: 0,
        expanded: expanded.then_some(0),
    });
    update(
        &mut app,
        Msg::Fetched(Request::Workload(key), Ok(Box::new(Payload::Workload(view)))),
        t(120),
    );
    render(&app, Theme { color: true }, 100, 30)
}

#[test]
fn an_expanded_stack_longer_than_the_room_is_cut_not_lost() {
    let lines = service_screen("billing", true);
    line_of(&lines, "job is already running");
    line_of(&lines, "at frame1");
    line_of(&lines, "more lines");
    assert!(!lines.iter().any(|l| l.contains("at frame40")));
}

#[test]
fn stderr_groups_count_no_failures_for_good() {
    line_of(&service_screen("billing", false), "(180 for good)");
    assert!(!service_screen("_stderr", false).iter().any(|l| l.contains("for good")));
}

#[test]
fn on_a_very_narrow_terminal_the_sections_and_the_fold_still_read() {
    let lines = overview_screen(Theme { color: true }, 70);
    line_of(&lines, "ONGOING");
    line_of(&lines, " fresh ");
}

#[test]
fn a_service_hides_its_muted_problems_unless_asked() {
    use crate::app::{Frame, Key, Screen};
    let key = skym_core::subject::WorkloadKey {
        host: "x".into(),
        project: "app".into(),
        service: "api".into(),
    };
    let view: skym_core::view::WorkloadView = serde_json::from_value(serde_json::json!({
        "key": key, "status": "ok", "facts": null,
        "state": { "run": "running", "exit_code": null, "health": null, "memory_used_bytes": null, "datastore": null },
        "incidents": [{ "subject": "workload:x/app/api", "code": "WORKLOAD_UNHEALTHY", "severity": "critical",
            "detail": "known flaky check", "opened_at": null, "open_for": null, "resolved_at": null,
            "muted": true, "mute_reason": "known" }]
    }))
    .unwrap();
    let mut app = App::default();
    app.stack.push(Frame { screen: Screen::Workload(key.clone()), cursor: 0, expanded: None });
    update(
        &mut app,
        Msg::Fetched(Request::Workload(key), Ok(Box::new(Payload::Workload(view)))),
        t(120),
    );
    let shows = |app: &App| {
        render(app, Theme { color: true }, 100, 30).iter().any(|l| l.contains("known flaky check"))
    };
    assert!(!shows(&app), "as on the overview, muted problems stay out of sight");
    update(&mut app, Msg::Key(Key::Char('m')), t(121));
    assert!(shows(&app));
}

#[test]
fn endpoints_show_under_the_hosts_and_among_the_problems() {
    let lines = screen(Theme { color: false });
    let shop = line_of(&lines, "shop.examp…");
    assert!(line_of(&lines, " x ") < shop, "after the customer's hosts");
    assert!(lines[shop].contains('✗') && lines[shop].contains('—'), "down, no answer");
    assert!(lines[line_of(&lines, "api.exampl…")].contains("1.2s"), "how fast it answered");
    let problem = &lines[line_of(&lines, " endpoint ")];
    assert!(problem.contains("shop.example.com") && problem.contains("ENDPOINT_DOWN detail"));
    assert!(line_of(&lines, " endpoint ") < line_of(&lines, "ONGOING"), "a new problem");
    line_of(&lines, "✗ 2 critical   ✓ 1 ok");
}
