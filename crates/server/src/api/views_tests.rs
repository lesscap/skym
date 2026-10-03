use super::*;
use crate::config::{Customer, HostEntry};
use skym_core::model::EventKind;
use skym_core::rules::{IncidentCode, Severity};

fn at(min: i64) -> Timestamp {
    Timestamp::from_second(1_790_000_000 + min * 60).unwrap()
}

fn incident(subject: &str, code: IncidentCode, severity: Severity) -> Incident {
    Incident {
        id: Some(1),
        host: None,
        subject: subject.parse().unwrap(),
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

#[test]
fn only_a_stopped_workload_knows_when_its_problem_began() {
    let key: WorkloadKey =
        WorkloadKey { host: "x".into(), project: "app".into(), service: "api".into() };
    let stopped = BTreeMap::from([(key, at(-600))]);
    let view = |code| {
        incident_view(
            &incident("workload:x/app/api", code, Severity::Critical),
            &[],
            &stopped,
            at(5),
        )
    };
    assert_eq!(view(IncidentCode::WorkloadDown).since, Some(at(-600)));
    assert_eq!(view(IncidentCode::WorkloadUnhealthy).since, None, "started is not unhealthy since");
    let other = incident("workload:x/app/web", IncidentCode::WorkloadDown, Severity::Critical);
    assert_eq!(incident_view(&other, &[], &stopped, at(5)).since, None);
    let mut resolved =
        incident("workload:x/app/api", IncidentCode::WorkloadDown, Severity::Critical);
    resolved.state = State::Resolved;
    resolved.resolved_at = Some(at(4));
    assert_eq!(incident_view(&resolved, &[], &stopped, at(5)).since, None, "only while open");
}

#[test]
fn mutes_match_exactly_and_expire() {
    let i = incident("workload:x/app/api", IncidentCode::WorkloadUnhealthy, Severity::Critical);
    let view = |mutes: &[Mute], now| incident_view(&i, mutes, &BTreeMap::new(), now);
    let exact = mute("workload:x/app/api", IncidentCode::WorkloadUnhealthy, Some(at(60)));
    let muted = view(std::slice::from_ref(&exact), at(30));
    assert_eq!((muted.muted, muted.mute_reason.as_deref()), (true, Some("known")));
    assert!(!view(&[exact], at(60)).muted, "expired at `until`");
    assert!(
        !view(&[mute("host:x", IncidentCode::WorkloadUnhealthy, None)], at(30)).muted,
        "a host mute does not cover its workloads"
    );
    assert!(!view(&[mute("workload:x/app/api", IncidentCode::WorkloadDown, None)], at(30)).muted);
    assert_eq!(view(&[], at(72)).open_for.as_deref(), Some("1h12m"));
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

#[test]
fn overview_ranks_hosts_and_ignores_muted_incidents() {
    let host = |id: &str| HostEntry {
        id: id.into(),
        customer: "acme".into(),
        token_sha256: String::new(),
    };
    let cfg = ServerConfig {
        customers: vec![Customer { id: "acme".into(), name: "Acme".into() }],
        hosts: vec![host("a"), host("b"), host("c"), host("d")],
        ..ServerConfig::default()
    };
    let seen: BTreeMap<String, Seen> = ["a", "b", "c"]
        .into_iter()
        .map(|h| (h.to_string(), Seen { first: at(-60), last: at(0) }))
        .collect();
    let open = [
        incident_view(
            &incident("host:a", IncidentCode::OomKilled, Severity::Warn),
            &[],
            &BTreeMap::new(),
            at(5),
        ),
        incident_view(
            &incident("workload:b/app/api", IncidentCode::WorkloadDown, Severity::Critical),
            &[],
            &BTreeMap::new(),
            at(5),
        ),
        incident_view(
            &incident("workload:c/app/api", IncidentCode::WorkloadDown, Severity::Critical),
            &[mute("workload:c/app/api", IncidentCode::WorkloadDown, None)],
            &BTreeMap::new(),
            at(5),
        ),
        incident_view(
            &incident("host:c", IncidentCode::LogUnbounded, Severity::Info),
            &[],
            &BTreeMap::new(),
            at(5),
        ),
    ];
    let o = overview(&cfg, &seen, &open, at(5));
    let hosts: Vec<(&str, Status)> =
        o.customers[0].hosts.iter().map(|h| (h.id.as_str(), h.status)).collect();
    assert_eq!(
        hosts,
        [("b", Status::Critical), ("d", Status::Unknown), ("a", Status::Warn), ("c", Status::Ok)]
    );
    assert_eq!(
        (o.status, o.customers[0].status, o.muted_count),
        (Status::Critical, Status::Critical, 1)
    );
    assert_eq!(o.customers[0].hosts[0].last_report_ago.as_deref(), Some("5m"));
    assert_eq!(o.customers[0].hosts[1].last_report_ago, None);
    let c = &o.customers[0].hosts[3];
    assert_eq!(
        (c.status, c.info_count, c.incidents.len()),
        (Status::Ok, 1, 1),
        "info is listed, not counted"
    );
    assert_eq!(o.customers[0].hosts[0].observed_since, Some(at(-60)));
    let calm = overview(&cfg, &seen, &open[2..], at(5));
    assert_eq!(calm.status, Status::Unknown, "a host that never reported outranks warnings");
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
