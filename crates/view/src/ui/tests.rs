use super::*;
use crate::api::{Payload, Request};
use crate::app::{Frame, Msg, Screen, update};
use crate::apps::{Grouping, Sort};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use skym_core::fixtures;
use skym_core::rules::IncidentCode;
use skym_core::subject::{AppKey, Subject};
use skym_core::view::{
    AppList, AppSummary, DiskUse, EndpointOverview, HostOverview, HostView, IncidentView, Overview,
    WorkloadSummary,
};

fn t(min: i64) -> Timestamp {
    Timestamp::from_second(1_790_000_000 + min * 60).unwrap()
}

/// An open problem on a host skym watches since minute 0, attributed as the server does.
fn incident(subject: &str, code: IncidentCode, severity: Severity, opened: i64) -> IncidentView {
    let i = fixtures::incident(subject, code, severity);
    IncidentView {
        app: match &i.subject {
            Subject::Workload(k) => Some(AppKey::of(k)),
            Subject::Endpoint(_) => Some("external/partner".parse().unwrap()),
            _ => None,
        },
        detail: format!("{} detail", code.as_str()),
        opened_at: Some(t(opened)),
        observed_since: Some(t(0)),
        ..i
    }
}

fn host(id: &str, status: Status, disks: Vec<DiskUse>) -> HostOverview {
    HostOverview {
        status,
        observed_since: Some(t(0)),
        load_1m: Some(1.3),
        memory_used_bytes: Some(9_800_000_000),
        memory_total_bytes: Some(31_000_000_000),
        disks,
        apps: 23,
        apps_in_trouble: 3,
        ..fixtures::host_overview(id)
    }
}

fn overview() -> Overview {
    Overview {
        ts: t(0),
        status: Status::Critical,
        muted_count: 0,
        problems: vec![
            incident("workload:x/app/old", IncidentCode::WorkloadUnhealthy, Severity::Critical, 1),
            incident("workload:x/app/fresh", IncidentCode::WorkloadDown, Severity::Critical, 100),
            incident("mount:x:/data", IncidentCode::DiskFilling, Severity::Warn, 1),
            incident(
                "endpoint:https://partner.example.com/",
                IncidentCode::EndpointDown,
                Severity::Critical,
                110,
            ),
            incident("host:x", IncidentCode::LogUnbounded, Severity::Info, 1),
        ],
        hosts: vec![
            host(
                "x",
                Status::Critical,
                vec![
                    DiskUse {
                        path: "/".into(),
                        used_percent: 36,
                        filling: false,
                        total_bytes: 0,
                        free_bytes: 0,
                        inodes_percent: 0,
                        fs_type: None,
                    },
                    DiskUse {
                        path: "/data".into(),
                        used_percent: 44,
                        filling: true,
                        total_bytes: 0,
                        free_bytes: 0,
                        inodes_percent: 0,
                        fs_type: None,
                    },
                ],
            ),
            host(
                "yi1",
                Status::Ok,
                vec![DiskUse {
                    path: "/".into(),
                    used_percent: 22,
                    filling: false,
                    total_bytes: 0,
                    free_bytes: 0,
                    inodes_percent: 0,
                    fs_type: None,
                }],
            ),
        ],
    }
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
        app: None,
    }
}

/// The problems tab, `width` columns wide.
fn problems_screen(theme: Theme, width: u16) -> Vec<String> {
    // The list alone; the preview has its own tests.
    let mut app = App { preview: false, ..App::default() };
    update(
        &mut app,
        Msg::Fetched(Request::Overview, Ok(Box::new(Payload::Overview(overview())))),
        t(120),
    );
    render(&app, theme, width, 16)
}

fn screen(theme: Theme) -> Vec<String> {
    problems_screen(theme, 100)
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
    assert!(line_of(&lines, "NEW") < line_of(&lines, "fresh: "));
    assert!(line_of(&lines, "fresh: ") < line_of(&lines, "ONGOING"));
    assert!(line_of(&lines, "ONGOING") < line_of(&lines, "old: "));
    assert!(lines[line_of(&lines, "old: ")].contains("≥2h"), "found at the first look");
    line_of(&lines, "1 hygiene items");
    assert!(!lines.iter().any(|l| l.contains("LOG_UNBOUNDED detail")), "folded");
}

#[test]
fn every_problem_names_its_app_or_its_host() {
    let lines = screen(Theme { color: false });
    let fresh = &lines[line_of(&lines, "fresh: ")];
    assert!(fresh.contains(" app ") && fresh.contains(" x "), "{fresh}");
    let disk = &lines[line_of(&lines, "/data: ")];
    assert!(disk.contains("host x"), "a host's own problem: {disk}");
    let url = &lines[line_of(&lines, "partner.example.com: ENDPOINT_DOWN detail")];
    assert!(url.contains(" partner ") && url.contains("external"), "{url}");
    line_of(&lines, "[Problems]");
}

#[test]
fn without_color_the_symbols_still_tell_the_status() {
    let lines = screen(Theme { color: false });
    assert!(lines[line_of(&lines, "fresh: ")].contains('✗'));
    let top = &lines[0];
    assert!(top.contains("✗ 3") && top.contains("! 1"), "problems by severity: {top}");
}

#[test]
fn on_a_narrow_terminal_the_reason_still_shows() {
    let lines = problems_screen(Theme { color: true }, 90);
    let row = &lines[line_of(&lines, "fresh: ")];
    assert!(row.contains("WORKLOAD_DOWN detail"), "the whole reason fits: {row}");
}

#[test]
fn on_a_very_narrow_terminal_the_sections_and_the_fold_still_read() {
    let lines = problems_screen(Theme { color: true }, 70);
    line_of(&lines, "ONGOING");
    line_of(&lines, "fresh: ");
    line_of(&lines, "hygiene items");
}

#[test]
fn the_hosts_tab_shows_load_disks_and_apps() {
    // The list alone; the preview has its own tests.
    let mut app = App { preview: false, ..App::default() };
    update(
        &mut app,
        Msg::Fetched(Request::Overview, Ok(Box::new(Payload::Overview(overview())))),
        t(120),
    );
    let warn = |key: &str| AppSummary {
        name: key.into(),
        status: Status::Warn,
        services: 1,
        running: 1,
        ..fixtures::app(key)
    };
    let apps = AppList { apps: vec![warn("x/a"), warn("x/b")] };
    update(&mut app, Msg::Fetched(Request::Apps, Ok(Box::new(Payload::Apps(apps)))), t(120));
    app.stack = vec![Frame { screen: Screen::Hosts, cursor: 0, expanded: None }];
    let hosts = &mut app.overview.value.as_mut().unwrap().hosts;
    (hosts[0].cpu_count, hosts[0].cpu_percent) = (Some(4), Some(23.4));
    hosts[1].cpu_count = Some(2);
    let lines = render(&app, Theme { color: false }, 120, 12);
    assert!(lines[line_of(&lines, " x ")].contains("(3 !)"), "marked by the worst app in trouble");
    assert!(lines[line_of(&lines, " x ")].contains(" 4 · 23% "), "CPUs and how busy");
    assert!(lines[line_of(&lines, " yi1 ")].contains(" 2 "), "no rate yet: the CPUs alone");
    let x = &lines[line_of(&lines, " x ")];
    assert!(x.contains("1.30") && x.contains("9.8 / 31.0 GB"), "{x}");
    assert!(x.contains("/data 44% ▲") && x.contains("23"), "{x}");
    assert!(line_of(&lines, " x ") < line_of(&lines, " yi1 "), "most urgent first");
    line_of(&lines, "[Hosts]");
}

#[test]
fn a_host_shows_its_own_problems_and_its_apps() {
    let mut view: HostView =
        serde_json::from_value(serde_json::json!({ "id": "x", "status": "critical" })).unwrap();
    view.incidents = vec![
        incident("mount:x:/data", IncidentCode::DiskFilling, Severity::Warn, 1),
        incident("workload:x/app/fresh", IncidentCode::WorkloadDown, Severity::Critical, 100),
    ];
    let app_summary = |key: &str, env: &str, status| AppSummary {
        env: Some(env.into()),
        configured: true,
        status,
        services: 2,
        running: 2,
        ..fixtures::app(key)
    };
    view.apps = vec![
        app_summary("x/app", "prod", Status::Critical),
        app_summary("x/blog", "test", Status::Ok),
    ];
    // The list alone; the preview has its own tests.
    let mut app = App { preview: false, ..App::default() };
    app.stack.push(Frame { screen: Screen::Host("x".into()), cursor: 0, expanded: None });
    update(
        &mut app,
        Msg::Fetched(Request::Host("x".into()), Ok(Box::new(Payload::Host(view)))),
        t(120),
    );
    let lines = render(&app, Theme { color: false }, 100, 20);
    line_of(&lines, "DISK_FILLING detail");
    assert!(
        !lines.iter().any(|l| l.contains("WORKLOAD_DOWN detail")),
        "the app's problem is the app's"
    );
    let apps = line_of(&lines, "Apps (2)");
    assert!(lines[line_of(&lines, " app ")].contains("prod") && line_of(&lines, " blog ") > apps);
    assert!(!lines.iter().any(|l| l.contains(" problems none")), "listed, so in their box");
    let calm = app.host.value.as_mut().unwrap();
    calm.incidents = vec![incident("host:x", IncidentCode::LogUnbounded, Severity::Info, 1)];
    let lines = render(&app, Theme { color: false }, 100, 20);
    line_of(&lines, " problems none · 1 hygiene items (h)");
    assert!(!lines.iter().any(|l| l.contains("┌ Problems")), "no box for nothing to list");
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
    // The list alone; the preview has its own tests.
    let mut app = App { preview: false, ..App::default() };
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
    // The list alone; the preview has its own tests.
    let mut app = App { preview: false, ..App::default() };
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
fn the_applications_page_groups_by_environment() {
    let app_summary = |key: &str, env: Option<&str>, status: Status, endpoints| AppSummary {
        env: env.map(String::from),
        configured: env.is_some(),
        status,
        services: 2,
        running: 1,
        last_deployed: Some(t(60)),
        endpoints,
        ..fixtures::app(key)
    };
    let list = AppList {
        apps: vec![
            AppSummary {
                workloads: vec![WorkloadSummary {
                    cpu_cores: Some(0.3),
                    ..fixtures::workload("y/shop/web")
                }],
                ..app_summary(
                    "y/shop",
                    Some("prod"),
                    Status::Ok,
                    vec![endpoint("https://shop.example.com/", Some(84), vec![])],
                )
            },
            app_summary("y/shop-test", Some("test"), Status::Ok, vec![]),
            app_summary("i/-/hbbs", None, Status::Ok, vec![]),
        ],
    };
    // The list alone; the preview has its own tests.
    let open = [(Grouping::Env, "prod".to_string())].into();
    let mut app = App { preview: false, open_groups: open, ..App::default() };
    update(&mut app, Msg::Key(crate::app::Key::Tab), t(120));
    update(&mut app, Msg::Fetched(Request::Apps, Ok(Box::new(Payload::Apps(list)))), t(120));
    let lines = render(&app, Theme { color: false }, 110, 14);
    let shop = line_of(&lines, " shop ");
    assert!(line_of(&lines, "▾ PROD") < shop && shop < line_of(&lines, "▸ TEST"));
    assert!(lines[line_of(&lines, "MEM")].contains("SVC   CPU   MEM"), "after the services");
    assert!(lines[shop].contains(" 0.30 "), "the cores its services keep busy");
    let narrow = render(&app, Theme { color: false }, 80, 14);
    let shop80 = &narrow[line_of(&narrow, " shop ")];
    assert!(shop80.contains("✓  shop ") && shop80.contains(" 0.30 "), "{shop80}");
    assert!(
        lines[shop].contains("shop.example.com  84ms") && lines[shop].ends_with("1h│"),
        "{}",
        lines[shop]
    );
    assert!(lines[shop].contains("1/2"), "running of all");
    assert!(lines[line_of(&lines, "▸ TEST")].contains("1 app "), "folded, all ok");
    assert!(!lines.iter().any(|l| l.contains(" shop-test ") || l.contains(" hbbs ")));
    line_of(&lines, "▸ UNCLASSIFIED");
    line_of(&lines, "Applications (3) · by environment");
    app.grouping = Grouping::Host;
    let lines = render(&app, Theme { color: false }, 100, 14);
    assert!(line_of(&lines, "▸ i ") < line_of(&lines, "▸ y "), "{lines:#?}");
}

#[test]
fn a_server_older_than_the_view_is_named_not_shown_as_calm() {
    let old: Overview = serde_json::from_value(serde_json::json!({
        "ts": "2026-10-01T00:00:00Z", "status": "critical",
        "customers": [{ "id": "acme", "name": "Acme", "status": "critical", "hosts": [] }]
    }))
    .unwrap();
    // The list alone; the preview has its own tests.
    let mut app = App { preview: false, ..App::default() };
    update(&mut app, Msg::Fetched(Request::Overview, Ok(Box::new(Payload::Overview(old)))), t(120));
    line_of(&render(&app, Theme { color: false }, 120, 8), "the server is older than this view");
    assert!(!screen(Theme { color: false })[0].contains("older"), "not with a current server");
}

/// The problems tab with its preview, the fresh problem selected.
fn previewed(width: u16, height: u16) -> Vec<String> {
    let mut o = overview();
    let fresh = o.problems.iter_mut().find(|p| p.subject.to_string().ends_with("fresh")).unwrap();
    fresh.detail =
        "exited (1), for 20m, no restart policy, and a reason long enough to be cut in the list"
            .into();
    fresh.workload = Some(Box::new(WorkloadSummary {
        status: Status::Critical,
        run: skym_core::model::RunState::Exited,
        exit_code: Some(1),
        image: Some("registry.example.com/fresh:1.4".into()),
        restart_policy: Some("unless-stopped".into()),
        ports: vec!["0.0.0.0:8080->80/tcp".into()],
        memory_used_bytes: Some(120_000_000),
        memory_limit_bytes: Some(512_000_000),
        restarts_last_hour: 2,
        ..fixtures::workload("x/app/fresh")
    }));
    o.hosts[0].os = Some("Ubuntu 22.04".into());
    o.hosts[0].ip = Some("203.0.113.7".into());
    let mut app = App::default();
    update(&mut app, Msg::Fetched(Request::Overview, Ok(Box::new(Payload::Overview(o)))), t(120));
    render(&app, Theme { color: false }, width, height)
}

#[test]
fn the_preview_tells_the_whole_problem_without_opening_it() {
    let lines = previewed(150, 30); // wide enough for each line, still under the list
    let list_end = line_of(&lines, "hygiene items");
    let full = line_of(&lines, "WORKLOAD_DOWN  exited (1), for 20m");
    assert!(full > list_end, "under the list");
    assert!(lines[full].contains("long enough to be cut in the list"), "in full");
    let preview = lines[full..].join(" ");
    let parts = [
        "fresh:1.4",
        "restart unless-stopped",
        "0.0.0.0:8080->80/tcp",
        "120M / 512M",
        "2 restarts",
    ];
    for part in parts {
        assert!(preview.contains(part), "{part} in {preview}");
    }
    let host = &lines[line_of(&lines, "203.0.113.7")];
    assert!(host.contains("Ubuntu 22.04") && host.contains("/data 44%"), "{host}");
}

#[test]
fn on_a_wide_terminal_the_preview_sits_beside_the_list() {
    let lines = previewed(170, 20);
    let row = line_of(&lines, "fresh: ");
    assert!(
        lines[..row + 3].iter().any(|l| l.contains("registry.example.com/fresh:1.4")),
        "beside, not under"
    );
}

/// A workload on x using this much memory.
fn using(bytes: u64) -> WorkloadSummary {
    WorkloadSummary { memory_used_bytes: Some(bytes), ..fixtures::workload("x/p/s") }
}

#[test]
fn the_apps_and_hosts_tabs_preview_their_selection() {
    let mut a = AppSummary {
        name: "Shop".into(),
        env: Some("prod".into()),
        note: Some("the web shop".into()),
        configured: true,
        status: Status::Warn,
        services: 1,
        running: 1,
        endpoints: vec![endpoint("https://shop.example.com/", Some(84), vec![])],
        deploys: vec![skym_core::view::Deploy {
            ts: t(60),
            service: "web".into(),
            from: "1.3".into(),
            to: "1.4".into(),
        }],
        exceptions_1h: 7,
        tags: vec!["acme".into(), "eu".into()],
        ..fixtures::app("x/shop")
    };
    a.endpoints[0].cert_expires_at = Some(t(60 * 24 * 30));
    let busy = WorkloadSummary { cpu_cores: Some(1.2), ..using(100_000_000) };
    a.workloads = vec![using(3_000_000_000), busy];
    let small = |name: &str, bytes| AppSummary {
        key: format!("x/{name}").parse().unwrap(),
        name: name.into(),
        status: Status::Ok,
        workloads: vec![using(bytes)],
        incidents: vec![],
        endpoints: vec![],
        ..a.clone()
    };
    let apps = vec![
        a.clone(),
        small("a1", 1_000_000_000),
        small("a2", 500_000_000),
        small("a3", 200_000_000),
        small("a4", 100_000_000),
    ];
    let mut app = App::default();
    update(
        &mut app,
        Msg::Fetched(Request::Overview, Ok(Box::new(Payload::Overview(overview())))),
        t(120),
    );
    update(
        &mut app,
        Msg::Fetched(Request::Apps, Ok(Box::new(Payload::Apps(AppList { apps })))),
        t(120),
    );
    app.stack = vec![Frame { screen: Screen::Apps, cursor: 0, expanded: None }];
    let lines = render(&app, Theme { color: false }, 120, 24);
    line_of(&lines, "5 apps · 4.9G · 1 in trouble · folded to its trouble");
    line_of(&lines, "apps     ! Shop · a1 · a2 · a3 · a4");
    assert!(
        lines[line_of(&lines, "▸ PROD")].contains("5 apps")
            && lines[line_of(&lines, "▸ PROD")].contains("4.9G")
    );
    assert!(lines[line_of(&lines, " Shop ")].contains("3.1G 10%"), "of x's 31G");
    app.sort = Sort::Memory;
    app.open_groups.insert((Grouping::Env, "prod".into()));
    let lines = render(&app, Theme { color: false }, 120, 24);
    line_of(&lines, "by environment · most memory first");
    assert!(lines[line_of(&lines, "MEM ▼")].contains("SVC   CPU   MEM ▼"));
    assert!(
        line_of(&lines, " Shop ") < line_of(&lines, " a1 ")
            && line_of(&lines, " a2 ") < line_of(&lines, " a3 ")
    );
    app.sort = Sort::Problems;
    app.open_groups.clear();
    app.stack[0].cursor = 1;
    let lines = render(&app, Theme { color: false }, 120, 36);
    line_of(&lines, "the web shop");
    assert!(lines[line_of(&lines, " tags ")].contains("acme · eu"));
    assert!(lines[line_of(&lines, "200 in 84ms")].contains("cert until"));
    assert!(lines[line_of(&lines, "web  1.3 → 1.4")].contains("1h ago"));
    line_of(&lines, "7 application exceptions in the last hour");
    app.stack = vec![Frame { screen: Screen::Hosts, cursor: 0, expanded: None }];
    let lines = render(&app, Theme { color: false }, 120, 30);
    assert!(lines[line_of(&lines, " cpu ")].contains("load 1.30   memory"), "no rates yet");
    assert!(!lines.iter().any(|l| l.contains(" net ")), "nor a network line");
    let x = &mut app.overview.value.as_mut().unwrap().hosts[0];
    (x.cpu_percent, x.iowait_percent, x.steal_percent) = (Some(23.4), Some(4.0), Some(0.4));
    (x.net_rx_bytes_per_s, x.net_tx_bytes_per_s) = (Some(1_200_000), Some(540));
    let lines = render(&app, Theme { color: false }, 120, 36);
    assert!(
        lines[line_of(&lines, " cpu ")].contains("23% busy · 4% iowait · load 1.30   memory"),
        "steal under 1% is left out"
    );
    line_of(&lines, " net      ↓ 1.2MB/s  ↑ 540B/s");
    assert!(
        lines[line_of(&lines, "top mem")].contains("Shop 3.1G · a1 1.0G · a2 500M · 2 others 300M"),
        "the three largest, then the rest"
    );
    let top_cpu = line_of(&lines, "top cpu  Shop 1.20");
    assert!(top_cpu > line_of(&lines, "top mem"), "the first to go when there is no room");
    let data = &lines[line_of(&lines, "/data ")..];
    assert!(data.iter().any(|l| l.contains("filling up")), "the disk in full, flagged");
}
