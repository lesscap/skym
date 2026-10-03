use super::*;
use skym_core::rules::IncidentCode;
use skym_core::view::{CustomerOverview, HostOverview, Status};
use std::collections::BTreeMap;

fn t(min: i64) -> Timestamp {
    Timestamp::from_second(1_790_000_000 + min * 60).unwrap()
}

fn incident(subject: &str, severity: Severity, opened: i64) -> IncidentView {
    IncidentView {
        subject: subject.parse().unwrap(),
        code: IncidentCode::WorkloadDown,
        severity,
        detail: "d".into(),
        opened_at: Some(t(opened)),
        since: None,
        open_for: None,
        resolved_at: None,
        muted: false,
        mute_reason: None,
        links: BTreeMap::new(),
    }
}

/// Host `x` watched since minute 0, host `y` since minute 60.
fn overview(x: Vec<IncidentView>, y: Vec<IncidentView>) -> Overview {
    let host = |id: &str, watched: i64, incidents| HostOverview {
        id: id.into(),
        status: Status::Critical,
        last_report_ago: Some("5s".into()),
        observed_since: Some(t(watched)),
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
            hosts: vec![host("x", 0, x), host("y", 60, y)],
        }],
        muted_count: 0,
    }
}

fn subjects(rows: &[Row]) -> Vec<String> {
    rows.iter().map(|r| r.incident.subject.to_string()).collect()
}

#[test]
fn new_means_after_the_baseline_and_within_a_day() {
    let o = overview(
        vec![
            incident("workload:x/a/zfound", Severity::Critical, 2), // at the first look
            incident("workload:x/a/fresh", Severity::Warn, 600),
            incident("workload:x/a/stale", Severity::Critical, 300),
            incident("workload:x/a/edge", Severity::Warn, 5), // just at the baseline
        ],
        vec![],
    );
    let now = t(300 + 24 * 60); // the stale one is exactly a day old: no longer new
    let p = problems(&o, &[], None, now);
    assert_eq!(subjects(&p.new), ["workload:x/a/fresh"]);
    // found at the first look: at least as old as skym's watch, so it comes first
    assert_eq!(
        subjects(&p.ongoing),
        ["workload:x/a/zfound", "workload:x/a/stale", "workload:x/a/edge"]
    );
    let edge = p.ongoing.iter().find(|r| r.incident.subject.to_string().ends_with("edge")).unwrap();
    assert_eq!(edge.age, Age::Exact(now.duration_since(t(5))), "after the baseline: its own age");
}

#[test]
fn age_is_exact_when_known_and_a_lower_bound_when_it_predates_skym() {
    let mut stopped = incident("workload:y/a/db", Severity::Critical, 61);
    stopped.since = Some(t(-1000));
    let o = overview(
        vec![incident("workload:x/a/found", Severity::Warn, 1)],
        vec![stopped, incident("workload:y/a/later", Severity::Warn, 100)],
    );
    let now = t(120);
    let p = problems(&o, &[], None, now);
    let age_of = |s: &str| {
        p.new.iter().chain(&p.ongoing).find(|r| r.incident.subject.to_string() == s).unwrap().age
    };
    assert_eq!(age_of("workload:y/a/db"), Age::Exact(SignedDuration::from_mins(1120)));
    assert_eq!(age_of("workload:x/a/found"), Age::AtLeast(SignedDuration::from_mins(120)));
    assert_eq!(age_of("workload:y/a/later"), Age::Exact(SignedDuration::from_mins(20)));
}

#[test]
fn worst_then_longest_first_with_hygiene_apart_and_a_host_filter() {
    let mut info = incident("host:x", Severity::Info, 1);
    info.code = IncidentCode::LogUnbounded;
    let o = overview(
        vec![
            incident("workload:x/a/warn", Severity::Warn, 1),
            incident("workload:x/a/short", Severity::Critical, 50),
            info,
        ],
        vec![incident("workload:y/a/long", Severity::Critical, 61)],
    );
    let p = problems(&o, &[], None, t(240));
    assert_eq!(subjects(&p.new), ["workload:x/a/short"], "opened after the baseline");
    assert_eq!(subjects(&p.ongoing), ["workload:y/a/long", "workload:x/a/warn"], "critical first");
    assert_eq!(subjects(&p.info), ["host:x"]);
    let only_x = problems(&o, &[], Some("x"), t(240));
    assert!(only_x.ongoing.iter().chain(&only_x.info).all(|r| r.host == "x"));
}

#[test]
fn muted_incidents_join_only_when_given() {
    let o = overview(vec![], vec![]);
    let mut quiet = incident("workload:x/a/legacy", Severity::Critical, 1);
    quiet.muted = true;
    let unmuted = incident("workload:x/a/other", Severity::Critical, 1);
    let given = [quiet, unmuted];
    let mut elsewhere = incident("workload:y/a/legacy", Severity::Critical, 61);
    elsewhere.muted = true;
    let given = [given[0].clone(), given[1].clone(), elsewhere];
    let p = problems(&o, &given, Some("x"), t(70));
    assert_eq!(subjects(&p.ongoing), ["workload:x/a/legacy"], "the overview already has unmuted");
    assert!(problems(&o, &[], None, t(10)).ongoing.is_empty());
}
