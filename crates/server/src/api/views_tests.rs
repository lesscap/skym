use super::*;
use crate::config::{Customer, Endpoint, Headers, HostEntry};
use crate::probe::Probe;
use skym_core::model::EventKind;
use skym_core::rules::{IncidentCode, Severity};

fn at(min: i64) -> Timestamp {
    Timestamp::from_second(1_790_000_000 + min * 60).unwrap()
}

fn incident(subject: &str, code: IncidentCode, severity: Severity) -> Incident {
    let subject: Subject = subject.parse().unwrap();
    Incident {
        id: Some(1),
        host: subject.host().cloned(),
        subject,
        code,
        state: State::Open,
        severity,
        peak_severity: severity,
        detail: "d".into(),
        match_streak: 2,
        clear_streak: 0,
        first_match_at: at(0),
        opened_at: Some(at(0)),
        last_seen: at(10),
        resolved_at: None,
    }
}

fn mute(subject: &str, code: IncidentCode, until: Option<Timestamp>) -> Mute {
    Mute { subject: subject.parse().unwrap(), code, reason: Some("known".into()), until }
}

fn probe(url: &str, app: &str) -> Endpoint {
    Endpoint {
        url: url.into(),
        expect: vec![],
        headers: Headers::default(),
        app: app.parse().unwrap(),
    }
}

/// Views of `incidents` with nothing stopped, muted or watched unless given.
struct Views {
    mutes: Vec<Mute>,
    stopped: BTreeMap<WorkloadKey, Timestamp>,
    probed: Vec<Endpoint>,
    hosts_seen: BTreeMap<HostId, Timestamp>,
    urls_seen: BTreeMap<String, Timestamp>,
}

impl Views {
    fn new() -> Self {
        Views {
            mutes: vec![],
            stopped: BTreeMap::new(),
            probed: vec![],
            hosts_seen: BTreeMap::new(),
            urls_seen: BTreeMap::new(),
        }
    }

    fn of(&self, i: &Incident, now: Timestamp) -> IncidentView {
        let cx = Context {
            mutes: &self.mutes,
            stopped: &self.stopped,
            probed: &self.probed,
            hosts_seen: &self.hosts_seen,
            urls_seen: &self.urls_seen,
            now,
        };
        incident_view(i, &cx)
    }
}

#[test]
fn only_a_stopped_workload_knows_when_its_problem_began() {
    let key = WorkloadKey { host: "x".into(), project: "app".into(), service: "api".into() };
    let views = Views { stopped: BTreeMap::from([(key, at(-600))]), ..Views::new() };
    let view = |code| views.of(&incident("workload:x/app/api", code, Severity::Critical), at(5));
    assert_eq!(view(IncidentCode::WorkloadDown).since, Some(at(-600)));
    assert_eq!(view(IncidentCode::WorkloadUnhealthy).since, None, "started is not unhealthy since");
    let other = incident("workload:x/app/web", IncidentCode::WorkloadDown, Severity::Critical);
    assert_eq!(views.of(&other, at(5)).since, None);
    let mut resolved =
        incident("workload:x/app/api", IncidentCode::WorkloadDown, Severity::Critical);
    resolved.state = State::Resolved;
    resolved.resolved_at = Some(at(4));
    assert_eq!(views.of(&resolved, at(5)).since, None, "only while open");
}

#[test]
fn mutes_match_exactly_and_expire() {
    let i = incident("workload:x/app/api", IncidentCode::WorkloadUnhealthy, Severity::Critical);
    let view = |mutes: Vec<Mute>, now| Views { mutes, ..Views::new() }.of(&i, now);
    let exact = mute("workload:x/app/api", IncidentCode::WorkloadUnhealthy, Some(at(60)));
    let muted = view(vec![exact.clone()], at(30));
    assert_eq!((muted.muted, muted.mute_reason.as_deref()), (true, Some("known")));
    assert!(!view(vec![exact], at(60)).muted, "expired at `until`");
    assert!(
        !view(vec![mute("host:x", IncidentCode::WorkloadUnhealthy, None)], at(30)).muted,
        "a host mute does not cover its workloads"
    );
    assert!(
        !view(vec![mute("workload:x/app/api", IncidentCode::WorkloadDown, None)], at(30)).muted
    );
    assert_eq!(view(vec![], at(72)).open_for.as_deref(), Some("1h12m"));
}

#[test]
fn problems_belong_to_their_app_else_their_host_and_know_since_when_they_are_watched() {
    let views = Views {
        probed: vec![probe("https://shop.example.com/", "x/shop")],
        hosts_seen: BTreeMap::from([("x".to_string(), at(-100))]),
        urls_seen: BTreeMap::from([("https://shop.example.com/".to_string(), at(-30))]),
        ..Views::new()
    };
    let view = |s: &str| views.of(&incident(s, IncidentCode::WorkloadDown, Severity::Warn), at(5));
    let app = |s: &str| view(s).app.map(|a| a.to_string());
    assert_eq!(app("workload:x/shop/web"), Some("x/shop".into()));
    assert_eq!(app("workload:x/-/redis"), Some("x/-/redis".into()));
    assert_eq!(app("app:x/gone"), Some("x/gone".into()));
    assert_eq!(app("endpoint:https://shop.example.com/"), Some("x/shop".into()));
    assert_eq!(app("endpoint:https://elsewhere.example.com/"), None, "not a probe any more");
    assert_eq!((app("host:x"), app("mount:x:/data")), (None, None), "a host's own");
    assert_eq!(view("workload:x/shop/web").links["app"], "/api/apps/x/shop");
    assert_eq!(view("endpoint:https://shop.example.com/").links["app"], "/api/apps/x/shop");
    assert!(!view("host:x").links.contains_key("app"));
    assert_eq!(view("workload:x/shop/web").observed_since, Some(at(-100)), "its host's");
    assert_eq!(
        view("endpoint:https://shop.example.com/").observed_since,
        Some(at(-30)),
        "its probe's"
    );
    assert_eq!(view("workload:y/shop/web").observed_since, None, "never heard from");
}

#[test]
fn links_encode_replica_names() {
    let l = links(&"workload:x/app/api#2".parse().unwrap());
    assert_eq!(l["workload"], "/api/hosts/x/workloads/app/api%232");
    assert_eq!(l["timeline"], "/api/timeline?host=x&workload=app/api%232&since=6h");
    assert_eq!(l["exceptions"], "/api/exceptions?host=x&workload=app/api%232&since=1h");
    let mount = links(&"mount:x:/data".parse().unwrap());
    assert_eq!(
        (mount["host"].as_str(), mount["incidents"].as_str()),
        ("/api/hosts/x", "/api/incidents?host=x")
    );
    assert_eq!(mount["exceptions"], "/api/exceptions?host=x&since=1h");
    assert!(links(&"endpoint:https://a.example".parse().unwrap()).is_empty());
    assert_eq!(encode("a b/c~d"), "a%20b%2Fc~d");
}

fn row(id: &str, seen: i64) -> HostRow {
    let r = skym_core::fixtures::full_report();
    HostRow {
        id: id.into(),
        facts: r.host_facts,
        state: r.host_state,
        errors: vec![],
        last_report_ts: at(seen),
        last_seen: at(seen),
        first_seen: at(-60),
        remote_addr: None,
    }
}

fn summary(key: &str, status: Status) -> AppSummary {
    AppSummary {
        key: key.parse().unwrap(),
        name: key.into(),
        env: None,
        note: None,
        configured: false,
        status,
        services: 1,
        running: 1,
        last_deployed: None,
        endpoints: vec![],
        incidents: vec![],
        links: BTreeMap::new(),
        workloads: vec![],
        deploys: vec![],
        exceptions_1h: 0,
        tags: vec![],
    }
}

#[test]
fn the_overview_lists_problems_flat_and_hosts_by_urgency() {
    let host = |id: &str, customer: Option<&str>| HostEntry {
        id: id.into(),
        customer: customer.map(String::from),
        token_sha256: String::new(),
        tags: vec![],
    };
    let cfg = ServerConfig {
        customers: vec![Customer { id: "acme".into(), name: "Acme".into() }],
        hosts: vec![host("a", Some("acme")), host("b", None), host("c", None), host("d", None)],
        ..ServerConfig::default()
    };
    let rows = [row("a", 0), row("b", 0), row("c", 0)];
    let views = Views {
        mutes: vec![mute("workload:c/app/api", IncidentCode::WorkloadDown, None)],
        ..Views::new()
    };
    let mut first = incident("host:a", IncidentCode::OomKilled, Severity::Warn);
    first.opened_at = Some(at(-5));
    let open: Vec<IncidentView> = [
        incident("workload:b/app/api", IncidentCode::WorkloadDown, Severity::Critical),
        first,
        incident("workload:c/app/api", IncidentCode::WorkloadDown, Severity::Critical),
        incident("host:c", IncidentCode::LogUnbounded, Severity::Info),
        incident("app:external/partner", IncidentCode::EndpointDown, Severity::Critical),
    ]
    .iter()
    .map(|i| views.of(i, at(5)))
    .collect();
    let apps = [
        summary("b/app", Status::Critical),
        summary("b/other", Status::Ok),
        summary("a/web", Status::Ok),
    ];
    let o = overview(&cfg, &rows, &apps, &open, at(5));
    let problems: Vec<String> = o.problems.iter().map(|p| p.subject.to_string()).collect();
    assert_eq!(
        problems,
        ["workload:b/app/api", "app:external/partner", "host:a", "host:c"],
        "worst first, then oldest; muted left out"
    );
    let hosts: Vec<(&str, Status)> = o.hosts.iter().map(|h| (h.id.as_str(), h.status)).collect();
    assert_eq!(
        hosts,
        [("b", Status::Critical), ("d", Status::Unknown), ("a", Status::Warn), ("c", Status::Ok)]
    );
    assert_eq!((o.status, o.muted_count), (Status::Critical, 1));
    let b = &o.hosts[0];
    assert_eq!((b.apps, b.apps_in_trouble), (2, 1));
    assert_eq!(b.last_report_ago.as_deref(), Some("5m"));
    assert_eq!(o.hosts[1].last_report_ago, None, "never reported");
    assert_eq!(
        (o.hosts[3].info_count, o.hosts[3].incidents.len()),
        (1, 1),
        "info listed, not counted"
    );
    assert_eq!(o.customers.len(), 1, "older views still get their grouping");
    let acme: Vec<&str> = o.customers[0].hosts.iter().map(|h| h.id.as_str()).collect();
    assert_eq!(acme, ["a"]);
}

#[test]
fn a_host_overview_shows_its_resources_and_which_disk_fills_up() {
    let mut r = row("x", 0);
    let mount = r.state.mounts[0].clone();
    r.state.mounts.push(skym_core::model::MountState { path: "/backup".into(), ..mount.clone() });
    let open: Vec<IncidentView> = [
        incident(&format!("mount:x:{}", mount.path), IncidentCode::DiskFilling, Severity::Warn),
        incident("mount:x:/backup", IncidentCode::OomKilled, Severity::Warn), // not filling
        incident("mount:x:/elsewhere", IncidentCode::DiskFilling, Severity::Warn), // no such disk
    ]
    .iter()
    .map(|i| Views::new().of(i, at(5)))
    .collect();
    let h = host_overview(&"x".to_string(), &[], Some(&r), &[], &open, at(5));
    assert_eq!(h.load_1m, Some(r.state.load_1m));
    assert_eq!(h.memory_used_bytes, Some(r.state.memory_used_bytes));
    assert_eq!(h.memory_total_bytes, r.facts.as_ref().map(|f| f.memory_total_bytes));
    let percent = |part: u64, whole: u64| (part * 100 / whole) as u8;
    assert_eq!(
        h.disks[0],
        DiskUse {
            path: mount.path.clone(),
            used_percent: percent(mount.used_bytes, mount.total_bytes),
            filling: true,
            total_bytes: mount.total_bytes,
            free_bytes: mount.total_bytes - mount.used_bytes,
            inodes_percent: percent(mount.inodes_used, mount.inodes_total),
            fs_type: r
                .facts
                .as_ref()
                .and_then(|f| f.mounts.iter().find(|m| m.path == mount.path))
                .map(|m| m.fs_type.clone()),
        }
    );
    assert!(h.disks[0].fs_type.is_some(), "the fixture names its file system");
    let facts = r.facts.as_ref().unwrap();
    assert_eq!(
        (h.os.as_deref(), h.kernel.as_deref(), h.cpu_count, h.boot_time),
        (
            Some(facts.os.as_str()),
            Some(facts.kernel.as_str()),
            Some(facts.cpu_count),
            Some(facts.boot_time)
        )
    );
    assert_eq!(
        (h.arch.as_deref(), h.agent_version.as_deref()),
        (Some(facts.arch.as_str()), Some(facts.agent_version.as_str()))
    );
    assert_eq!(h.docker_version, facts.docker_version);
    assert!(h.disks[1..].iter().all(|d| !d.filling), "only that disk, only for DISK_FILLING");
    assert_eq!(h.disks.last().map(|d| d.path.as_str()), Some("/backup"));
    let mut seen = r.clone();
    seen.remote_addr = Some("203.0.113.7".into());
    assert_eq!(
        host_overview(&"x".to_string(), &[], Some(&seen), &[], &[], at(5)).ip.as_deref(),
        Some("203.0.113.7")
    );
    let tags = ["cn".to_string(), "acme".to_string(), "cn".to_string()];
    let silent = host_overview(&"y".to_string(), &tags, None, &[], &[], at(5));
    assert_eq!((silent.status, silent.disks.len(), silent.load_1m), (Status::Unknown, 0, None));
    assert_eq!(silent.tags, ["acme", "cn"], "sorted, once each, even before it reports");
}

#[test]
fn an_endpoint_says_how_it_answered_unless_its_probes_stopped() {
    let e = probe("https://a.example", "x/shop");
    let row = |minutes_ago: i64, response: Result<u16, String>| ProbeRow {
        url: e.url.clone(),
        first_seen: at(-60),
        probe: Probe {
            at: at(5 - minutes_ago),
            response,
            latency_ms: 120,
            cert_not_after: Some(at(60 * 24 * 30)),
        },
    };
    let interval = SignedDuration::from_secs(60);
    let fresh = endpoint(&e, Some(&row(1, Ok(200))), &[], interval, at(5));
    assert_eq!(
        (fresh.status, fresh.http_status, fresh.latency_ms, fresh.app.map(|a| a.to_string())),
        (Status::Ok, Some(200), Some(120), Some("x/shop".into()))
    );
    assert_eq!(endpoint(&e, Some(&row(3, Ok(200))), &[], interval, at(5)).status, Status::Ok);
    assert_eq!(endpoint(&e, Some(&row(4, Ok(200))), &[], interval, at(5)).status, Status::Unknown);
    assert_eq!(endpoint(&e, None, &[], interval, at(5)).status, Status::Unknown, "never probed");
    let silent = endpoint(&e, Some(&row(1, Err("timeout".into()))), &[], interval, at(5));
    assert_eq!((silent.http_status, silent.latency_ms, silent.cert_expires_at), (None, None, None));
}

#[test]
fn cap_marks_only_real_cuts() {
    assert_eq!(cap(vec![1, 2, 3], 3), (vec![1, 2, 3], false));
    assert_eq!(cap(vec![1, 2, 3], 2), (vec![1, 2], true));
}

#[test]
fn timeline_merges_newest_first_and_marks_truncation() {
    let change = |min, change| LogEntry {
        ts: at(min),
        subject: "host:x".parse().unwrap(),
        code: IncidentCode::OomKilled,
        change,
        severity: Severity::Warn,
        detail: "d".into(),
    };
    let event =
        Event { ts: at(2), subject: "host:x".parse().unwrap(), kind: EventKind::HostRebooted };
    let t = timeline(vec![change(3, Change::Resolved), change(1, Change::Opened)], vec![event], 10);
    let order: Vec<i64> =
        t.entries.iter().map(|e| (e.ts.as_second() - 1_790_000_000) / 60).collect();
    assert_eq!((order, t.truncated), (vec![3, 2, 1], false));
    assert!(matches!(t.entries[1].entry, TimelineKind::Event { event: EventKind::HostRebooted }));
    let cut = timeline(vec![change(3, Change::Severity), change(1, Change::Reopened)], vec![], 1);
    assert_eq!((cut.entries.len(), cut.truncated), (1, true));
}
