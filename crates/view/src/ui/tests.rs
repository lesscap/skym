use super::*;
use crate::api::{Payload, Request};
use crate::app::{Msg, update};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use skym_core::rules::IncidentCode;
use skym_core::view::{CustomerOverview, HostOverview, IncidentView, Overview};
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
        }],
        muted_count: 0,
    };
    let mut app = App::default();
    update(
        &mut app,
        Msg::Fetched(Request::Overview, Ok(Box::new(Payload::Overview(overview)))),
        t(120),
    );
    let mut terminal = Terminal::new(TestBackend::new(100, 16)).unwrap();
    terminal.draw(|f| draw(f, &app, "https://skym.example.com", t(120), theme)).unwrap();
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
    line_of(&lines, "1 critical");
}
