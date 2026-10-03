use super::*;
use skym_core::rules::{IncidentCode, Severity};
use skym_core::view::{CustomerOverview, HostOverview, IncidentView, Status};
use std::collections::BTreeMap;

fn t(sec: i64) -> Timestamp {
    Timestamp::from_second(1_790_000_000 + sec).unwrap()
}

fn incident(subject: &str) -> IncidentView {
    IncidentView {
        subject: subject.parse().unwrap(),
        code: IncidentCode::WorkloadDown,
        severity: Severity::Critical,
        detail: "exited (1)".into(),
        opened_at: Some(t(0)),
        since: None,
        open_for: None,
        resolved_at: None,
        muted: false,
        mute_reason: None,
        links: BTreeMap::new(),
    }
}

fn overview() -> Overview {
    let host = |id: &str, incidents: Vec<IncidentView>| HostOverview {
        id: id.into(),
        status: Status::Critical,
        last_report_ago: Some("5s".into()),
        observed_since: Some(t(0)),
        info_count: 0,
        incidents,
        links: BTreeMap::new(),
    };
    Overview {
        ts: t(0),
        status: Status::Critical,
        customers: vec![CustomerOverview {
            id: "acme".into(),
            name: "Acme".into(),
            status: Status::Critical,
            hosts: vec![
                host("x", vec![incident("workload:x/app/api")]),
                host("y", vec![incident("host:y")]),
            ],
        }],
        muted_count: 0,
    }
}

/// An app that has its overview.
fn loaded() -> App {
    let mut app = App::default();
    update(
        &mut app,
        Msg::Fetched(Request::Overview, Ok(Box::new(Payload::Overview(overview())))),
        t(1),
    );
    app
}

fn press(app: &mut App, keys: &[Key]) -> Vec<Request> {
    keys.iter().flat_map(|k| update(app, Msg::Key(*k), t(2))).collect()
}

#[test]
fn the_first_tick_reads_everything_and_then_every_thirty_seconds() {
    let mut app = App::default();
    assert_eq!(update(&mut app, Msg::Tick, t(0)), [Request::Overview]);
    assert!(update(&mut app, Msg::Tick, t(29)).is_empty());
    assert_eq!(update(&mut app, Msg::Tick, t(30)), [Request::Overview]);
    press(&mut app, &[Key::Char('m')]);
    assert_eq!(
        update(&mut app, Msg::Key(Key::Char('r')), t(31)),
        [Request::Overview, Request::Muted]
    );
}

#[test]
fn opening_a_host_then_a_service_asks_for_them_and_back_returns() {
    let mut app = loaded();
    assert_eq!(press(&mut app, &[Key::Down, Key::Enter]), [Request::Host("x".into())]);
    assert_eq!(app.frame().screen, Screen::Host("x".into()));
    let key = WorkloadKey { host: "x".into(), project: "app".into(), service: "api".into() };
    assert!(press(&mut app, &[Key::Char('t')]).contains(&Request::Timeline {
        host: "x".into(),
        workload: None,
        since: "24h"
    }));
    assert_eq!(press(&mut app, &[Key::Char(']')]).len(), 1, "a wider window is read again");
    assert!(press(&mut app, &[Key::Char(']')]).is_empty(), "7d is the widest");
    press(&mut app, &[Key::Esc, Key::Esc]);
    assert_eq!(app.frame().screen, Screen::Overview);
    // the problems pane opens the problem's service directly
    assert_eq!(press(&mut app, &[Key::Tab, Key::Up, Key::Enter]), [Request::Workload(key)]);
    press(&mut app, &[Key::Esc, Key::Esc, Key::Esc]);
    assert_eq!(app.stack.len(), 1, "the overview stays");
}

#[test]
fn picking_a_host_narrows_the_problems() {
    let mut app = loaded();
    assert_eq!(app.problem_rows(t(2)).len(), 2);
    press(&mut app, &[Key::Down, Key::Down]);
    assert_eq!(app.picked_host().map(String::as_str), Some("y"));
    assert_eq!(app.problem_rows(t(2)).iter().map(|r| r.host).collect::<Vec<_>>(), ["y"]);
    press(&mut app, &[Key::Down]);
    assert_eq!(app.host_cursor, 2, "the last host is the bottom");
}

#[test]
fn a_failure_keeps_the_last_data_and_a_rejected_token_stops_the_view() {
    let mut app = loaded();
    let down = Err(FetchError::Unreachable("connection refused".into()));
    update(&mut app, Msg::Fetched(Request::Overview, down), t(40));
    assert!(app.overview.value.is_some() && app.overview.at == Some(t(1)));
    assert_eq!(app.error, Some(FetchError::Unreachable("connection refused".into())));
    update(
        &mut app,
        Msg::Fetched(Request::Overview, Ok(Box::new(Payload::Overview(overview())))),
        t(50),
    );
    assert_eq!((app.error.clone(), app.overview.at), (None, Some(t(50))));
    update(&mut app, Msg::Fetched(Request::Overview, Err(FetchError::Unauthorized)), t(60));
    assert!(app.quit && app.fatal.as_deref().is_some_and(|f| f.contains("token")));
}

#[test]
fn an_answer_for_a_screen_left_behind_is_dropped() {
    let mut app = loaded();
    press(&mut app, &[Key::Down, Key::Enter, Key::Esc]);
    let late = serde_json::from_value::<HostView>(serde_json::json!({ "id": "x", "status": "ok" }))
        .unwrap();
    update(
        &mut app,
        Msg::Fetched(Request::Host("x".into()), Ok(Box::new(Payload::Host(late)))),
        t(3),
    );
    assert!(app.host.value.is_none());
}

#[test]
fn filter_toggles_and_help() {
    let mut app = loaded();
    press(
        &mut app,
        &[Key::Tab, Key::Char('/'), Key::Char('A'), Key::Char('P'), Key::Char('I'), Key::Enter],
    );
    assert_eq!(app.filter.as_deref(), Some("API"));
    let rows = app.problem_rows(t(2));
    assert_eq!(rows.len(), 1, "case-insensitive, by the name shown");
    assert_eq!(app.host_ids().len(), 2, "the other pane is not filtered");
    press(&mut app, &[Key::Char('q')]);
    assert!(app.quit, "typing ended with enter, so q quits");
    let mut app = loaded();
    press(&mut app, &[Key::Char('/'), Key::Char('q'), Key::Backspace, Key::Esc]);
    assert!(!app.quit && app.filter.is_none());
    press(&mut app, &[Key::Char('?')]);
    assert!(app.help);
    press(&mut app, &[Key::Char('q')]);
    assert!(!app.help && !app.quit, "any key only closes help");
    assert_eq!(press(&mut app, &[Key::Char('m')]), [Request::Muted]);
    assert!(press(&mut app, &[Key::Char('m')]).is_empty());
    press(&mut app, &[Key::Char('h')]);
    assert!(app.show_info);
}

fn host_view() -> HostView {
    serde_json::from_value(serde_json::json!({
        "id": "x", "status": "critical",
        "workloads": [
            { "key": { "host": "x", "project": "app", "service": "web" }, "status": "ok", "run": "running" },
            { "key": { "host": "x", "project": "app", "service": "api" }, "status": "ok", "run": "running" },
            { "key": { "host": "x", "project": "zz", "service": "db" }, "status": "critical", "run": "exited" }
        ]
    }))
    .unwrap()
}

fn workload_view() -> WorkloadView {
    serde_json::from_value(serde_json::json!({
        "key": { "host": "x", "project": "app", "service": "api" }, "status": "warn",
        "facts": null, "state": { "run": "running", "exit_code": null, "health": null,
                                  "memory_used_bytes": null, "datastore": null },
        "exceptions": [
            { "workload": { "host": "x", "project": "app", "service": "api" }, "class": "application",
              "component": "c", "code": "A", "count": 1, "final_count": 1,
              "first_seen": "2026-10-01T00:00:00Z", "last_seen": "2026-10-01T00:00:00Z" },
            { "workload": { "host": "x", "project": "app", "service": "api" }, "class": "application",
              "component": "c", "code": "B", "count": 1, "final_count": 1,
              "first_seen": "2026-10-01T00:00:00Z", "last_seen": "2026-10-01T00:00:00Z" }
        ]
    }))
    .unwrap()
}

#[test]
fn a_hosts_services_list_problems_first_then_by_name() {
    let mut app = loaded();
    press(&mut app, &[Key::Down, Key::Enter]);
    update(
        &mut app,
        Msg::Fetched(Request::Host("x".into()), Ok(Box::new(Payload::Host(host_view())))),
        t(3),
    );
    let names =
        |app: &App| app.services().iter().map(|w| w.key.service.clone()).collect::<Vec<_>>();
    assert_eq!(names(&app), ["db", "api", "web"]);
    assert_eq!(
        press(
            &mut app,
            &[Key::Down, Key::Down, Key::Up, Key::Char('k'), Key::Char('j'), Key::Enter]
        )
        .len(),
        1
    );
    assert_eq!(
        app.frame().screen,
        Screen::Workload(WorkloadKey {
            host: "x".into(),
            project: "app".into(),
            service: "api".into()
        })
    );
    press(&mut app, &[Key::Esc, Key::Tab]);
    assert_eq!(app.pane, Pane::Hosts, "tab switches panes on the overview only");
    assert_eq!(press(&mut app, &[Key::Char('e')]), [Request::Exceptions("x".into())]);
}

#[test]
fn a_services_exceptions_expand_and_its_timeline_is_its_own() {
    let mut app = loaded();
    let key = WorkloadKey { host: "x".into(), project: "app".into(), service: "api".into() };
    press(&mut app, &[Key::Tab, Key::Enter]);
    update(
        &mut app,
        Msg::Fetched(
            Request::Workload(key.clone()),
            Ok(Box::new(Payload::Workload(workload_view()))),
        ),
        t(3),
    );
    press(&mut app, &[Key::Down, Key::Enter]);
    assert_eq!(app.frame().expanded, Some(1));
    press(&mut app, &[Key::Enter]);
    assert_eq!(app.frame().expanded, None, "enter again folds it");
    let asked = press(&mut app, &[Key::Char('t')]);
    assert_eq!(
        asked,
        [Request::Timeline { host: "x".into(), workload: Some(key.clone()), since: "24h" }]
    );
    assert_eq!(
        press(&mut app, &[Key::Char('[')]),
        [Request::Timeline { host: "x".into(), workload: Some(key), since: "6h" }]
    );
}

#[test]
fn moving_to_another_host_starts_its_problems_at_the_top() {
    let mut app = loaded();
    press(&mut app, &[Key::Tab, Key::Down]);
    assert_eq!(app.frame().cursor, 1, "moving among problems keeps the place");
    press(&mut app, &[Key::Tab, Key::Down]);
    assert_eq!(app.frame().cursor, 0);
}

#[test]
fn muted_problems_join_once_shown_and_read() {
    let mut app = loaded();
    let mut quiet = incident("workload:x/app/legacy");
    quiet.muted = true;
    let list = IncidentList { incidents: vec![quiet], truncated: false };
    press(&mut app, &[Key::Char('m')]);
    update(&mut app, Msg::Fetched(Request::Muted, Ok(Box::new(Payload::Muted(list)))), t(3));
    assert!(app.problem_rows(t(4)).iter().any(|r| r.incident.muted));
    press(&mut app, &[Key::Char('/'), Key::Char('x'), Key::Backspace, Key::Char('w'), Key::Enter]);
    assert_eq!(app.filter.as_deref(), Some("w"), "backspace removes the last letter");
}

#[test]
fn filtering_hosts_keeps_the_pick_within_the_list_and_ctrl_c_quits() {
    let mut app = loaded();
    press(&mut app, &[Key::Down, Key::Down]);
    assert_eq!(app.host_cursor, 2);
    press(&mut app, &[Key::Char('/'), Key::Char('x')]);
    assert_eq!(app.host_ids(), ["x"]);
    assert_eq!(app.host_cursor, 1, "clamped to the shorter list");
    press(&mut app, &[Key::Quit]);
    assert!(app.quit && app.filter.as_deref() == Some("x"), "quits even while typing");
}

#[test]
fn a_timeline_has_no_timeline_of_its_own() {
    let mut app = loaded();
    press(&mut app, &[Key::Down, Key::Enter, Key::Char('t')]);
    assert!(press(&mut app, &[Key::Char('t')]).is_empty());
    assert_eq!(app.stack.len(), 3);
}

#[test]
fn when_skym_started_watching_comes_from_the_overview() {
    let mut app = loaded();
    let hosts = &mut app.overview.value.as_mut().unwrap().customers[0].hosts;
    hosts[1].observed_since = Some(t(-60));
    assert_eq!(app.observed_since("y"), Some(t(-60)));
    assert_eq!(app.observed_since("x"), Some(t(0)));
    assert_eq!(app.observed_since("nowhere"), None);
    assert_eq!(App::default().observed_since("x"), None);
}
