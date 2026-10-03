use super::*;
use skym_core::rules::{IncidentCode, Severity};
use skym_core::subject::{AppKey, Subject};
use skym_core::view::{AppSummary, HostOverview, IncidentView, Status, WorkloadSummary};
use std::collections::BTreeMap;

fn t(sec: i64) -> Timestamp {
    Timestamp::from_second(1_790_000_000 + sec).unwrap()
}

/// An open, critical problem, attributed as the server does.
fn incident(subject: &str) -> IncidentView {
    let subject: Subject = subject.parse().unwrap();
    IncidentView {
        app: match &subject {
            Subject::Workload(k) => Some(AppKey::of(k)),
            Subject::App(a) => Some(a.clone()),
            Subject::Endpoint(_) => Some("x/shop".parse().unwrap()),
            _ => None,
        },
        subject,
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
        observed_since: Some(t(0)),
    }
}

fn host(id: &str) -> HostOverview {
    HostOverview {
        id: id.into(),
        status: Status::Critical,
        last_report_ago: Some("5s".into()),
        observed_since: Some(t(0)),
        info_count: 0,
        incidents: vec![],
        links: BTreeMap::new(),
        load_1m: Some(0.5),
        memory_used_bytes: None,
        memory_total_bytes: None,
        disks: vec![],
        apps: 1,
        apps_in_trouble: 1,
    }
}

/// Problems of four kinds, all as old and as bad, so listed by host then subject: a
/// workload, an endpoint, an app's own (all on x), then y's own.
fn overview() -> Overview {
    Overview {
        ts: t(0),
        status: Status::Critical,
        customers: vec![],
        muted_count: 0,
        problems: vec![
            incident("app:x/gone"),
            incident("endpoint:https://shop.example.com/"),
            incident("host:y"),
            incident("workload:x/app/api"),
        ],
        hosts: vec![host("x"), host("y")],
    }
}

fn summary(key: &str, env: Option<&str>, status: Status) -> AppSummary {
    AppSummary {
        key: key.parse().unwrap(),
        name: key.rsplit('/').next().unwrap().into(),
        env: env.map(String::from),
        note: None,
        configured: env.is_some(),
        status,
        services: 1,
        running: 1,
        last_deployed: None,
        endpoints: vec![],
        incidents: vec![],
        links: BTreeMap::new(),
    }
}

fn fetched(app: &mut App, request: Request, payload: Payload, at: Timestamp) {
    update(app, Msg::Fetched(request, Ok(Box::new(payload))), at);
}

/// An app on the problems tab with the overview and the applications.
fn loaded() -> App {
    let mut app = App::default();
    fetched(&mut app, Request::Overview, Payload::Overview(overview()), t(1));
    let mut shop = summary("x/shop", Some("prod"), Status::Critical);
    shop.name = "Shop".into();
    let apps = AppList { apps: vec![shop, summary("x/app", None, Status::Critical)] };
    fetched(&mut app, Request::Apps, Payload::Apps(apps), t(1));
    assert_eq!(app.frame().screen, Screen::Problems);
    app
}

fn press(app: &mut App, keys: &[Key]) -> Vec<Request> {
    keys.iter().flat_map(|k| update(app, Msg::Key(*k), t(2))).collect()
}

fn host_view() -> HostView {
    let mut v: HostView =
        serde_json::from_value(serde_json::json!({ "id": "x", "status": "critical" })).unwrap();
    v.apps =
        vec![summary("x/zz", Some("prod"), Status::Critical), summary("x/app", None, Status::Ok)];
    v
}

fn app_view(key: &str) -> AppView {
    let service = |name: &str, status| WorkloadSummary {
        key: WorkloadKey { host: "x".into(), project: "app".into(), service: name.into() },
        kind: None,
        status,
        run: skym_core::model::RunState::Running,
        exit_code: None,
        state_since: None,
        image: None,
        links: BTreeMap::new(),
    };
    AppView {
        app: summary(key, None, Status::Critical),
        workloads: vec![service("web", Status::Ok), service("api", Status::Critical)],
    }
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

/// Opens the problem at `row` of the problems tab.
fn open_problem(app: &mut App, row: usize) -> Vec<Request> {
    app.stack.truncate(1);
    app.frame_mut().cursor = row;
    press(app, &[Key::Enter])
}

#[test]
fn the_first_tick_reads_everything_and_then_every_thirty_seconds() {
    let mut app = App::default();
    assert_eq!(update(&mut app, Msg::Tick, t(0)), [Request::Overview, Request::Apps]);
    assert!(update(&mut app, Msg::Tick, t(29)).is_empty());
    press(&mut app, &[Key::Char('m')]);
    assert_eq!(
        update(&mut app, Msg::Key(Key::Char('r')), t(31)),
        [Request::Overview, Request::Apps, Request::Muted]
    );
}

#[test]
fn problems_are_named_by_their_app_and_open_where_they_belong() {
    let mut app = loaded();
    let names: Vec<String> = app.problem_rows(t(2)).iter().map(|r| app.app_name(r)).collect();
    assert_eq!(names, ["app", "Shop", "gone", "host y"], "the configured name when there is one");
    let api = WorkloadKey { host: "x".into(), project: "app".into(), service: "api".into() };
    assert_eq!(open_problem(&mut app, 0), [Request::Workload(api)]);
    assert_eq!(open_problem(&mut app, 1), [Request::App("x/shop".parse().unwrap())], "its app");
    assert_eq!(open_problem(&mut app, 2), [Request::App("x/gone".parse().unwrap())]);
    assert_eq!(open_problem(&mut app, 3), [Request::Host("y".into())]);
}

#[test]
fn tabs_take_turns_as_the_root_and_leave_what_was_open() {
    let mut app = loaded();
    open_problem(&mut app, 0);
    press(&mut app, &[Key::Char('/'), Key::Char('x'), Key::Enter]);
    assert_eq!(press(&mut app, &[Key::Tab]), [], "the applications are already read");
    assert_eq!((app.stack.len(), app.tab(), app.filter.clone()), (1, &Screen::Apps, None));
    press(&mut app, &[Key::Tab]);
    assert_eq!(app.tab(), &Screen::Hosts);
    press(&mut app, &[Key::Tab]);
    assert_eq!(app.tab(), &Screen::Problems);
    let mut cold = App::default();
    assert_eq!(press(&mut cold, &[Key::Tab]), [Request::Apps], "nothing read yet");
}

#[test]
fn a_host_lists_its_apps_and_an_app_its_services() {
    let mut app = loaded();
    press(&mut app, &[Key::Tab, Key::Tab]);
    assert_eq!(app.host_rows().len(), 2);
    assert_eq!(press(&mut app, &[Key::Enter]), [Request::Host("x".into())]);
    fetched(&mut app, Request::Host("x".into()), Payload::Host(host_view()), t(3));
    let apps: Vec<String> = app.host_apps().iter().map(|a| a.name.clone()).collect();
    assert_eq!(apps, ["zz", "app"], "in the server's order: problems first");
    let zz: AppKey = "x/zz".parse().unwrap();
    assert_eq!(press(&mut app, &[Key::Enter]), [Request::App(zz.clone())]);
    fetched(&mut app, Request::App(zz), Payload::App(app_view("x/zz")), t(3));
    let services: Vec<String> = app.services().iter().map(|w| w.key.service.clone()).collect();
    assert_eq!(services, ["api", "web"], "problems first");
    let api = WorkloadKey { host: "x".into(), project: "app".into(), service: "api".into() };
    assert_eq!(press(&mut app, &[Key::Enter]), [Request::Workload(api)]);
    press(&mut app, &[Key::Esc, Key::Esc, Key::Esc]);
    assert_eq!((app.stack.len(), app.tab()), (1, &Screen::Hosts));
}

#[test]
fn a_failure_keeps_the_last_data_and_a_rejected_token_stops_the_view() {
    let mut app = loaded();
    let down = Err(FetchError::Unreachable("connection refused".into()));
    update(&mut app, Msg::Fetched(Request::Overview, down), t(40));
    assert!(app.overview.value.is_some() && app.overview.at == Some(t(1)));
    assert_eq!(app.error, Some(FetchError::Unreachable("connection refused".into())));
    fetched(&mut app, Request::Overview, Payload::Overview(overview()), t(50));
    assert_eq!((app.error.clone(), app.overview.at), (None, Some(t(50))));
    update(&mut app, Msg::Fetched(Request::Overview, Err(FetchError::Unauthorized)), t(60));
    assert!(app.quit && app.fatal.as_deref().is_some_and(|f| f.contains("token")));
}

#[test]
fn an_answer_for_a_screen_left_behind_is_dropped() {
    let mut app = loaded();
    open_problem(&mut app, 3);
    press(&mut app, &[Key::Esc]);
    let late = serde_json::from_value::<HostView>(serde_json::json!({ "id": "y", "status": "ok" }))
        .unwrap();
    fetched(&mut app, Request::Host("y".into()), Payload::Host(late), t(3));
    assert!(app.host.value.is_none());
}

#[test]
fn filter_toggles_and_help() {
    let mut app = loaded();
    press(&mut app, &[Key::Char('/'), Key::Char('S'), Key::Char('H'), Key::Char('O'), Key::Enter]);
    assert_eq!(app.filter.as_deref(), Some("SHO"));
    assert_eq!(app.problem_rows(t(2)).len(), 1, "case-insensitive, by the app's name");
    press(&mut app, &[Key::Char('q')]);
    assert!(app.quit, "typing ended with enter, so q quits");
    let mut app = loaded();
    press(&mut app, &[Key::Char('/'), Key::Char('q'), Key::Backspace, Key::Esc]);
    assert!(!app.quit && app.filter.is_none());
    press(&mut app, &[Key::Char('/'), Key::Char('y')]);
    assert_eq!(app.problem_rows(t(2)).len(), 1, "by host too");
    press(&mut app, &[Key::Quit]);
    assert!(app.quit, "ctrl-c quits even while typing");
    let mut app = loaded();
    press(&mut app, &[Key::Char('?')]);
    assert!(app.help);
    press(&mut app, &[Key::Char('q')]);
    assert!(!app.help && !app.quit, "any key only closes help");
    assert_eq!(press(&mut app, &[Key::Char('m')]), [Request::Muted]);
    assert!(press(&mut app, &[Key::Char('m')]).is_empty());
    press(&mut app, &[Key::Char('h')]);
    assert!(app.show_info);
}

#[test]
fn a_services_exceptions_expand_and_its_timeline_is_its_own() {
    let mut app = loaded();
    let key = WorkloadKey { host: "x".into(), project: "app".into(), service: "api".into() };
    open_problem(&mut app, 0);
    fetched(&mut app, Request::Workload(key.clone()), Payload::Workload(workload_view()), t(3));
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
        [Request::Timeline { host: "x".into(), workload: Some(key.clone()), since: "6h" }]
    );
    assert_eq!(
        press(&mut app, &[Key::Char(']')]),
        [Request::Timeline { host: "x".into(), workload: Some(key.clone()), since: "24h" }]
    );
    assert!(press(&mut app, &[Key::Char('t')]).is_empty(), "a timeline has none of its own");
}

#[test]
fn muted_problems_join_once_shown_and_read() {
    let mut app = loaded();
    let mut quiet = incident("workload:x/legacy/worker");
    quiet.muted = true;
    press(&mut app, &[Key::Char('m')]);
    let list = IncidentList { incidents: vec![quiet], truncated: false };
    fetched(&mut app, Request::Muted, Payload::Muted(list), t(3));
    assert_eq!(app.problem_rows(t(3)).len(), 5);
    press(&mut app, &[Key::Char('m')]);
    assert_eq!(app.problem_rows(t(3)).len(), 4);
}

#[test]
fn the_applications_tab_folds_environments_and_leads_to_an_app() {
    let mut app = loaded();
    let list = AppList {
        apps: vec![
            summary("y/shop", Some("prod"), Status::Ok),
            summary("y/shop-test", Some("test"), Status::Ok),
            summary("x/blog", Some("test"), Status::Critical),
        ],
    };
    fetched(&mut app, Request::Apps, Payload::Apps(list), t(2));
    press(&mut app, &[Key::Tab]);
    let names = |app: &App| app.app_rows().iter().map(|a| a.name.clone()).collect::<Vec<_>>();
    assert_eq!(names(&app), ["shop", "blog"], "test is folded, its trouble still shown");
    press(&mut app, &[Key::Char('e')]);
    assert_eq!(names(&app), ["shop", "blog", "shop-test"]);
    press(&mut app, &[Key::Char('/'), Key::Char('b'), Key::Enter]);
    assert_eq!(names(&app), ["blog"]);
    let blog: AppKey = "x/blog".parse().unwrap();
    assert_eq!(press(&mut app, &[Key::Enter]), [Request::App(blog)]);
    assert_eq!(app.filter, None, "a new screen starts unfiltered");
}

#[test]
fn going_back_reads_a_screen_again_when_its_data_moved_on() {
    let mut app = loaded();
    let api = workload_view().key; // x/app/api
    let other = WorkloadKey { service: "web".into(), ..api.clone() };
    open_problem(&mut app, 0);
    fetched(&mut app, Request::Workload(api.clone()), Payload::Workload(workload_view()), t(3));
    app.stack.push(Frame { screen: Screen::Workload(other.clone()), cursor: 0, expanded: None });
    let view = WorkloadView { key: other.clone(), ..workload_view() };
    fetched(&mut app, Request::Workload(other), Payload::Workload(view), t(3));
    assert_eq!(press(&mut app, &[Key::Esc]), [Request::Workload(api)], "web's data, api's screen");
    assert!(app.workload.value.is_none(), "not shown meanwhile");
}

#[test]
fn going_back_to_the_same_screen_reads_nothing_again() {
    let mut app = loaded();
    press(&mut app, &[Key::Tab, Key::Tab, Key::Enter]);
    fetched(&mut app, Request::Host("x".into()), Payload::Host(host_view()), t(3));
    press(&mut app, &[Key::Enter]);
    assert!(press(&mut app, &[Key::Esc]).is_empty(), "the host's data is its own");
    assert!(app.host.value.is_some());
}

#[test]
fn going_back_to_a_screen_whose_answer_never_came_reads_it() {
    let mut app = loaded();
    let api = WorkloadKey { host: "x".into(), project: "app".into(), service: "api".into() };
    open_problem(&mut app, 0);
    press(&mut app, &[Key::Char('t')]); // before the service's answer: it will be dropped
    assert_eq!(press(&mut app, &[Key::Esc]), [Request::Workload(api)]);
}

#[test]
fn refreshing_the_applications_tab_reads_them_once() {
    let mut app = loaded();
    press(&mut app, &[Key::Tab]);
    assert_eq!(press(&mut app, &[Key::Char('r')]), [Request::Overview, Request::Apps]);
}
