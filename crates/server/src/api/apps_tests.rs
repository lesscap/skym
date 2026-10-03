use super::*;
use skym_core::model::WorkloadState;
use skym_core::rules::{IncidentCode, Severity};
use skym_core::view::Status;

fn at(min: i64) -> Timestamp {
    Timestamp::from_second(1_790_000_000 + min * 60).unwrap()
}

fn row(subject: &str, run: RunState) -> WorkloadRow {
    let Ok(Subject::Workload(key)) = subject.parse() else { unreachable!() };
    let state =
        WorkloadState { run, ..skym_core::fixtures::full_report().workloads[0].state.clone() };
    WorkloadRow { key, facts: None, state, last_seen: at(0) }
}

fn incident(subject: &str, code: IncidentCode, severity: Severity) -> IncidentView {
    IncidentView {
        subject: subject.parse().unwrap(),
        code,
        severity,
        detail: "d".into(),
        opened_at: Some(at(0)),
        since: None,
        open_for: None,
        resolved_at: None,
        muted: false,
        mute_reason: None,
        links: BTreeMap::new(),
        app: None,
        observed_since: None,
    }
}

fn configured(id: &str, name: Option<&str>) -> AppConfig {
    AppConfig {
        id: id.parse().unwrap(),
        name: name.map(String::from),
        env: Some("prod".into()),
        note: Some("the shop".into()),
        probes: vec![],
    }
}

fn probe(url: &str, app: &str) -> EndpointOverview {
    EndpointOverview {
        url: url.into(),
        status: Status::Critical,
        last_probe_ago: None,
        observed_since: None,
        http_status: None,
        latency_ms: None,
        cert_expires_at: None,
        incidents: vec![],
        app: Some(app.parse().unwrap()),
    }
}

#[test]
fn workloads_group_into_projects_and_lone_ones() {
    let rows = [
        row("workload:x/shop/web", RunState::Running),
        row("workload:x/shop/worker", RunState::Exited),
        row("workload:x/shop/cron", RunState::Exited),
        row("workload:x/-/redis", RunState::Running),
        row("workload:x/_systemd/xray", RunState::Running),
        row("workload:y/shop/web", RunState::Running),
    ];
    let deployed = BTreeMap::from([
        (rows[0].key.clone(), at(10)),
        (rows[1].key.clone(), at(30)),
        (rows[5].key.clone(), at(99)),
    ]);
    let apps = summaries(&rows, &[], &[], &[], &deployed);
    let keys: Vec<String> = apps.iter().map(|a| a.key.to_string()).collect();
    assert_eq!(keys, ["x/-/redis", "x/_systemd/xray", "x/shop", "y/shop"]);
    let shop = &apps[2];
    assert_eq!(
        (shop.name.as_str(), shop.services, shop.running, shop.last_deployed),
        ("shop", 3, 1, Some(at(30))),
        "its own deployments, the latest"
    );
    assert_eq!(
        (apps[0].name.as_str(), apps[0].configured, apps[0].env.clone()),
        ("redis", false, None)
    );
    assert_eq!(shop.links["app"], "/api/apps/x/shop");
    assert_eq!(apps[0].links["app"], "/api/apps/x/-/redis");
    assert!(apps.iter().all(|a| a.status == Status::Ok));
}

#[test]
fn incidents_and_probes_belong_to_their_app_and_rank_it() {
    let rows = [
        row("workload:x/shop/web", RunState::Running),
        row("workload:x/blog/web", RunState::Running),
    ];
    let mut muted = incident("workload:x/blog/web", IncidentCode::WorkloadDown, Severity::Critical);
    muted.muted = true;
    let open = [
        incident("workload:x/shop/web", IncidentCode::WorkloadUnhealthy, Severity::Warn),
        incident(
            "endpoint:https://shop.example.com/",
            IncidentCode::EndpointDown,
            Severity::Critical,
        ),
        incident(
            "endpoint:https://other.example.com/",
            IncidentCode::EndpointDown,
            Severity::Critical,
        ),
        incident("app:x/gone", IncidentCode::AppMissing, Severity::Critical),
        muted,
    ];
    let probes = [probe("https://shop.example.com/", "x/shop")];
    let config =
        [configured("x/shop", Some("Shop")), configured("x/gone", None), configured("x/new", None)];
    let apps = summaries(&rows, &config, &probes, &open, &BTreeMap::new());
    let ranked: Vec<(&str, Status, usize)> =
        apps.iter().map(|a| (a.name.as_str(), a.status, a.incidents.len())).collect();
    let subjects =
        |i: usize| apps[i].incidents.iter().map(|i| i.subject.to_string()).collect::<Vec<_>>();
    assert_eq!(subjects(1), ["workload:x/shop/web", "endpoint:https://shop.example.com/"]);
    assert_eq!(
        ranked,
        [
            ("gone", Status::Critical, 1),
            ("Shop", Status::Critical, 2),
            ("new", Status::Unknown, 0),
            ("blog", Status::Ok, 0),
        ],
        "a configured app with nothing yet is unknown; a muted incident does not count"
    );
    let shop = &apps[1];
    assert_eq!(
        (shop.endpoints.len(), shop.configured, shop.note.as_deref()),
        (1, true, Some("the shop"))
    );
    assert_eq!(shop.env.as_deref(), Some("prod"));
    assert_eq!((apps[0].services, apps[0].running), (0, 0));
}

#[test]
fn a_silent_host_takes_its_apps_with_it() {
    let rows = [
        row("workload:x/shop/web", RunState::Running),
        row("workload:y/blog/web", RunState::Running),
    ];
    let open = [
        incident("host:x", IncidentCode::HeartbeatLost, Severity::Critical),
        incident("host:y", IncidentCode::OomKilled, Severity::Warn),
    ];
    let apps = summaries(&rows, &[], &[], &open, &BTreeMap::new());
    let status: Vec<(&str, Status)> = apps.iter().map(|a| (a.name.as_str(), a.status)).collect();
    assert_eq!(
        status,
        [("shop", Status::Critical), ("blog", Status::Ok)],
        "other host problems stay the host's"
    );
}

#[test]
fn an_external_app_is_known_by_its_urls() {
    let mut partner = configured("external/partner", Some("Partner"));
    partner.probes = vec![];
    let answering =
        EndpointOverview { status: Status::Ok, ..probe("https://p.example/", "external/partner") };
    let apps = summaries(&[], std::slice::from_ref(&partner), &[answering], &[], &BTreeMap::new());
    assert_eq!((apps[0].status, apps[0].services), (Status::Ok, 0), "no services, yet fine");
    let unprobed = EndpointOverview {
        status: Status::Unknown,
        ..probe("https://p.example/", "external/partner")
    };
    let apps = summaries(&[], &[partner], &[unprobed], &[], &BTreeMap::new());
    assert_eq!(apps[0].status, Status::Unknown, "not probed yet");
}
