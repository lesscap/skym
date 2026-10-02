//! Evolution: older and newer peers must keep understanding each other.

use skym_core::model::{
    DatastoreKind, ExceptionClass, Health, LocalEvent, RunState, WorkloadFacts, WorkloadKind,
};
use skym_core::report::{Report, ReportError, facts_hash, rehost, validate};
use skym_core::subject::{Subject, WorkloadKey};
use skym_core::view::HostView;

fn load(name: &str) -> Report {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn full_report_round_trips() {
    let report = load("report-full.json");
    let json = serde_json::to_string(&report).unwrap();
    assert_eq!(serde_json::from_str::<Report>(&json).unwrap(), report);
}

#[test]
fn newer_peer_values_are_tolerated() {
    let report = load("report-future.json");
    let w = &report.workloads[0];
    let facts = w.facts.as_ref().unwrap();
    assert_eq!(facts.kind, WorkloadKind::Unknown);
    assert_eq!(facts.datastore, Some(DatastoreKind::Unknown));
    assert_eq!(w.state.run, RunState::Unknown);
    assert_eq!(w.state.health, Some(Health::Unknown));
    assert_eq!(report.local_events, vec![LocalEvent::Unknown]);
    assert_eq!(report.exceptions[0].class, ExceptionClass::Unknown);
}

#[test]
fn older_peer_minimal_report_parses() {
    let json = r#"{
        "host": "x", "ts": "2026-10-01T12:00:00Z", "host_facts_hash": "0000000000000000",
        "host_state": { "load_1m": 0, "load_5m": 0, "load_15m": 0, "memory_used_bytes": 0 }
    }"#;
    let report: Report = serde_json::from_str(json).unwrap();
    assert!(report.host_facts.is_none());
    assert!(report.workloads.is_empty() && report.local_events.is_empty());
    assert!(report.host_state.mounts.is_empty());
}

#[test]
fn subject_strings_round_trip_and_reject_malformed() {
    let valid = [
        "host:i",
        "workload:i/dify/weaviate",
        "workload:i/_systemd/getty@tty1.service",
        "mount:i:/data:x",
        "endpoint:https://x.example.com:8443/a",
    ];
    for s in valid {
        let subject: Subject = s.parse().unwrap();
        assert_eq!(subject.to_string(), s);
        let json = serde_json::to_string(&subject).unwrap();
        assert_eq!(serde_json::from_str::<Subject>(&json).unwrap(), subject);
    }
    let invalid = [
        "host:",
        "host:a/b",
        "workload:i/a",
        "workload:/p/s",
        "workload:i//s",
        "workload:i/p/",
        "workload:a:b/p/s",
        "mount:i:data",
        "endpoint:",
        "foo:x",
        "x",
    ];
    for s in invalid {
        assert!(s.parse::<Subject>().is_err(), "{s} should be rejected");
    }
}

#[test]
fn facts_hash_is_stable_and_sensitive() {
    let facts = load("report-full.json").workloads[0].facts.clone().unwrap();
    let hash = facts_hash(&facts);
    assert_eq!(hash.len(), 16);
    assert!(hash.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));

    let reordered: WorkloadFacts = {
        let mut labels: Vec<_> = facts.labels.clone().into_iter().collect();
        labels.reverse();
        WorkloadFacts { labels: labels.into_iter().collect(), ..facts.clone() }
    };
    assert_eq!(facts_hash(&reordered), hash);

    let changed = WorkloadFacts { image: "registry.example.com/captain:1.4.3".into(), ..facts };
    assert_ne!(facts_hash(&changed), hash);

    let schema = |s: schemars::Schema| serde_json::to_value(s).unwrap();
    assert!(schema(schemars::schema_for!(Report))["properties"]["workloads"].is_object());
    let host_view = schema(schemars::schema_for!(HostView));
    assert!(host_view["properties"]["incidents"].is_object());
    assert_eq!(host_view["$defs"]["Subject"]["type"], "string");
}

#[test]
fn rehost_rewrites_every_host_reference() {
    let mut report = load("report-full.json");
    let key = report.workloads[0].key.clone();
    report.local_events = vec![
        LocalEvent::OomKilled { ts: report.ts, workload: Some(key) },
        LocalEvent::OomKilled { ts: report.ts, workload: None },
        LocalEvent::Unknown,
    ];
    let rehosted = rehost(report.clone(), "server-id");

    let json = serde_json::to_value(&rehosted).unwrap();
    let hosts: Vec<&serde_json::Value> = json["workloads"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| &w["key"]["host"])
        .chain(json["exceptions"].as_array().unwrap().iter().map(|g| &g["workload"]["host"]))
        .chain(std::iter::once(&json["local_events"][0]["workload"]["host"]))
        .chain(std::iter::once(&json["host"]))
        .collect();
    assert_eq!(hosts.len(), report.workloads.len() + report.exceptions.len() + 2);
    assert!(hosts.iter().all(|h| *h == "server-id"));

    let restored = rehost(rehosted, &report.host);
    assert_eq!(restored, report, "nothing but host references changed");
}

#[test]
fn run_state_display_matches_wire_names() {
    use RunState::*;
    for state in [Running, Restarting, Paused, Exited, Dead, Created, Inactive, Unknown] {
        assert_eq!(serde_json::to_value(state).unwrap(), state.to_string());
    }
}

#[test]
fn valid_workload_keys_survive_the_subject_string() {
    let key = |host: &str, project: &str, service: &str| WorkloadKey {
        host: host.into(),
        project: project.into(),
        service: service.into(),
    };
    let valid =
        [key("i", "dify", "api"), key("i", "-", "a/b"), key("i", "_systemd", "getty@tty1.service")];
    for k in valid {
        assert!(k.is_valid(), "{k:?}");
        let s = Subject::Workload(k.clone()).to_string();
        assert_eq!(s.parse::<Subject>().unwrap(), Subject::Workload(k));
    }
    let invalid = [
        key("", "p", "s"),
        key("a:b", "p", "s"),
        key("i", "", "s"),
        key("i", "a/b", "s"),
        key("i", "p", ""),
    ];
    assert!(invalid.iter().all(|k| !k.is_valid()));
}

#[test]
fn validate_rejects_invalid_and_duplicate_workloads() {
    let report = load("report-full.json");
    assert_eq!(validate(&report), Ok(()));

    let mut duplicate = report.clone();
    duplicate.workloads.push(report.workloads[0].clone());
    let key = report.workloads[0].key.clone();
    assert_eq!(validate(&duplicate), Err(ReportError::DuplicateWorkloadKey(key)));

    let bad = WorkloadKey { project: String::new(), ..report.workloads[0].key.clone() };
    let mut in_workloads = report.clone();
    in_workloads.workloads[0].key = bad.clone();
    let mut in_events = report.clone();
    in_events.local_events =
        vec![LocalEvent::OomKilled { ts: report.ts, workload: Some(bad.clone()) }];
    let mut in_exceptions = report.clone();
    in_exceptions.exceptions[0].workload = bad.clone();
    for r in [in_workloads, in_events, in_exceptions] {
        assert_eq!(validate(&r), Err(ReportError::InvalidWorkloadKey(bad.clone())));
    }
}
