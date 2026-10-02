//! Evolution: older and newer peers must keep understanding each other.

use skym_core::model::{
    DatastoreKind, ExceptionClass, Health, LocalEvent, RunState, WorkloadFacts, WorkloadKind,
};
use skym_core::report::{Report, facts_hash};
use skym_core::subject::Subject;
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
    assert!(
        hash.chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    );

    let reordered: WorkloadFacts = {
        let mut labels: Vec<_> = facts.labels.clone().into_iter().collect();
        labels.reverse();
        WorkloadFacts {
            labels: labels.into_iter().collect(),
            ..facts.clone()
        }
    };
    assert_eq!(facts_hash(&reordered), hash);

    let changed = WorkloadFacts {
        image: "registry.example.com/captain:1.4.3".into(),
        ..facts
    };
    assert_ne!(facts_hash(&changed), hash);

    let schema = |s: schemars::Schema| serde_json::to_value(s).unwrap();
    assert!(schema(schemars::schema_for!(Report))["properties"]["workloads"].is_object());
    assert!(schema(schemars::schema_for!(HostView))["properties"]["incidents"].is_object());
}
