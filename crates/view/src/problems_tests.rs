use super::*;
use skym_core::fixtures;
use skym_core::rules::IncidentCode;
use skym_core::view::Status;

fn t(min: i64) -> Timestamp {
    Timestamp::from_second(1_790_000_000 + min * 60).unwrap()
}

/// An open problem; skym watches host `x` since minute 0 and host `y` since minute 60.
fn incident(subject: &str, severity: Severity, opened: i64) -> IncidentView {
    let subject: skym_core::subject::Subject = subject.parse().unwrap();
    let watched = match subject.host().map(String::as_str) {
        Some("x") => Some(t(0)),
        Some("y") => Some(t(60)),
        _ => None,
    };
    let i = fixtures::incident(&subject.to_string(), IncidentCode::WorkloadDown, severity);
    IncidentView {
        app: match &subject {
            skym_core::subject::Subject::Workload(k) => Some(AppKey::of(k)),
            _ => None,
        },
        detail: "d".into(),
        opened_at: Some(t(opened)),
        observed_since: watched,
        ..i
    }
}

/// The overview with these problems (the order does not matter).
fn overview(x: Vec<IncidentView>, y: Vec<IncidentView>) -> Overview {
    Overview {
        ts: t(0),
        status: Status::Critical,
        muted_count: 0,
        problems: x.into_iter().chain(y).collect(),
        hosts: vec![],
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
    let p = problems(&o, &[], now);
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
    let p = problems(&o, &[], now);
    let age_of = |s: &str| {
        p.new.iter().chain(&p.ongoing).find(|r| r.incident.subject.to_string() == s).unwrap().age
    };
    assert_eq!(age_of("workload:y/a/db"), Age::Exact(SignedDuration::from_mins(1120)));
    assert_eq!(age_of("workload:x/a/found"), Age::AtLeast(SignedDuration::from_mins(120)));
    assert_eq!(age_of("workload:y/a/later"), Age::Exact(SignedDuration::from_mins(20)));
}

#[test]
fn worst_then_longest_first_with_hygiene_apart() {
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
    let p = problems(&o, &[], t(240));
    assert_eq!(subjects(&p.new), ["workload:x/a/short"], "opened after the baseline");
    assert_eq!(subjects(&p.ongoing), ["workload:y/a/long", "workload:x/a/warn"], "critical first");
    assert_eq!(subjects(&p.info), ["host:x"]);
    assert_eq!(p.info[0].app(), None, "a host's own problem");
    assert_eq!(p.new[0].app().map(ToString::to_string), Some("x/a".into()));
}

#[test]
fn a_url_runs_where_its_app_does_and_is_dated_from_its_first_probe() {
    let mut down = incident("endpoint:https://partner.example.com/", Severity::Critical, 100);
    down.app = Some("external/partner".parse().unwrap());
    down.observed_since = Some(t(99));
    let mut ours = incident("endpoint:https://shop.example.com/", Severity::Warn, 2);
    ours.app = Some("y/shop".parse().unwrap());
    ours.observed_since = Some(t(0));
    let o = overview(vec![down, ours], vec![]);
    let p = problems(&o, &[], t(120));
    let row = |s: &str| {
        p.new.iter().chain(&p.ongoing).find(|r| r.incident.subject.to_string().contains(s)).unwrap()
    };
    assert_eq!((row("partner").host, row("shop").host), ("external", "y"));
    assert_eq!(
        row("partner").age,
        Age::AtLeast(SignedDuration::from_mins(21)),
        "found at its first probe"
    );
    assert_eq!(
        subjects(&p.ongoing),
        ["endpoint:https://partner.example.com/", "endpoint:https://shop.example.com/"]
    );
}

#[test]
fn muted_incidents_join_only_when_given() {
    let o = overview(vec![incident("workload:x/a/other", Severity::Critical, 1)], vec![]);
    let mut quiet = incident("workload:x/a/legacy", Severity::Critical, 1);
    quiet.muted = true;
    let given = [quiet, incident("workload:x/a/other", Severity::Critical, 1)];
    let p = problems(&o, &given, t(70));
    assert_eq!(
        subjects(&p.ongoing),
        ["workload:x/a/legacy", "workload:x/a/other"],
        "the overview already has the unmuted one"
    );
    assert_eq!(problems(&o, &[], t(70)).ongoing.len(), 1);
}
