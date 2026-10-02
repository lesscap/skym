//! Judgement rules: each test starts from a healthy report and breaks one thing.

use skym_core::judge::{JudgeInput, Recent, judge};
use skym_core::model::{
    DatastoreProbe, ExceptionClass, ExceptionGroup, Health, LocalEvent, RunState,
};
use skym_core::report::Report;
use skym_core::rules::{IncidentCode, Severity};
use skym_core::subject::{Subject, WorkloadKey};
use skym_core::time::{SignedDuration, Timestamp};
use skym_core::view::{IncidentView, Status, rollup};
use std::collections::{BTreeMap, BTreeSet};

const APP: usize = 0;
const PG: usize = 1;
const XRAY: usize = 2;

fn base() -> Report {
    let path = format!(
        "{}/tests/fixtures/report-full.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[derive(Default)]
struct Ctx {
    oom: Vec<LocalEvent>,
    exceptions: Vec<ExceptionGroup>,
    open: BTreeSet<(Subject, IncidentCode)>,
}

fn run(report: &Report, ctx: &Ctx) -> Vec<(Subject, IncidentCode, Severity)> {
    let facts = BTreeMap::new();
    let input = JudgeInput {
        report,
        facts: &facts,
        recent: Recent {
            oom_events: &ctx.oom,
            exceptions: &ctx.exceptions,
        },
        open: &ctx.open,
    };
    judge(&input)
        .into_iter()
        .map(|f| (f.subject, f.code, f.severity))
        .collect()
}

fn severity_of(report: &Report, ctx: &Ctx, code: IncidentCode) -> Option<Severity> {
    run(report, ctx)
        .into_iter()
        .find(|(_, c, _)| *c == code)
        .map(|(_, _, s)| s)
}

fn key(i: usize) -> WorkloadKey {
    base().workloads[i].key.clone()
}

fn ago(report: &Report, mins: i64) -> Timestamp {
    report.ts - SignedDuration::from_mins(mins)
}

fn open(subject: Subject, code: IncidentCode) -> Ctx {
    Ctx {
        open: BTreeSet::from([(subject, code)]),
        ..Ctx::default()
    }
}

#[test]
fn healthy_report_has_no_findings() {
    assert!(run(&base(), &Ctx::default()).is_empty());
}

#[test]
fn workload_down() {
    let check = |run_state, exit_code, missing: Vec<u16>, i: usize| {
        let mut r = base();
        r.workloads[i].state.run = run_state;
        r.workloads[i].state.exit_code = exit_code;
        r.workloads[i].state.missing_ports = missing;
        severity_of(&r, &Ctx::default(), IncidentCode::WorkloadDown)
    };
    assert_eq!(
        check(RunState::Exited, Some(1), vec![], APP),
        Some(Severity::Critical)
    );
    assert_eq!(
        check(RunState::Restarting, None, vec![], APP),
        Some(Severity::Critical)
    );
    assert_eq!(check(RunState::Exited, Some(0), vec![], APP), None);
    assert_eq!(check(RunState::Unknown, None, vec![], APP), None);
    assert_eq!(
        check(RunState::Running, None, vec![443], XRAY),
        Some(Severity::Critical)
    );
}

#[test]
fn crash_loop_with_hysteresis() {
    let with_restarts = |mins: &[i64]| {
        let mut r = base();
        r.workloads[APP].state.restarts = mins.iter().map(|m| ago(&r, *m)).collect();
        r
    };
    let none = Ctx::default();
    let is_open = open(Subject::Workload(key(APP)), IncidentCode::CrashLoop);
    assert_eq!(
        severity_of(&with_restarts(&[1, 2]), &none, IncidentCode::CrashLoop),
        None
    );
    let three = with_restarts(&[1, 2, 3]);
    assert_eq!(
        severity_of(&three, &none, IncidentCode::CrashLoop),
        Some(Severity::Warn)
    );
    let ten = with_restarts(&[1; 10]);
    assert_eq!(
        severity_of(&ten, &none, IncidentCode::CrashLoop),
        Some(Severity::Critical)
    );
    let one_recent = with_restarts(&[20]);
    assert_eq!(
        severity_of(&one_recent, &is_open, IncidentCode::CrashLoop),
        Some(Severity::Warn)
    );
    let one_old = with_restarts(&[40]);
    assert_eq!(
        severity_of(&one_old, &is_open, IncidentCode::CrashLoop),
        None
    );
}

#[test]
fn oom_kills_are_grouped_per_subject() {
    let r = base();
    let event = |workload| LocalEvent::OomKilled {
        ts: ago(&r, 5),
        workload,
    };
    let ctx = Ctx {
        oom: vec![
            event(Some(key(APP))),
            event(Some(key(APP))),
            event(Some(key(APP))),
            event(None),
        ],
        ..Ctx::default()
    };
    let found = run(&r, &ctx);
    assert!(found.contains(&(
        Subject::Workload(key(APP)),
        IncidentCode::OomKilled,
        Severity::Critical
    )));
    assert!(found.contains(&(
        Subject::Host("x".into()),
        IncidentCode::OomKilled,
        Severity::Warn
    )));
    assert_eq!(found.len(), 2);
}

#[test]
fn disk_filling_with_hysteresis_and_inodes() {
    let with = |used_pct: u64, inodes_used: u64, inodes_total: u64| {
        let mut r = base();
        let m = &mut r.host_state.mounts[0];
        (m.used_bytes, m.total_bytes) = (used_pct, 100);
        (m.inodes_used, m.inodes_total) = (inodes_used, inodes_total);
        r
    };
    let none = Ctx::default();
    let mount = Subject::Mount {
        host: "x".into(),
        path: "/".into(),
    };
    let is_open = open(mount, IncidentCode::DiskFilling);
    let disk = |r: &Report, ctx: &Ctx| severity_of(r, ctx, IncidentCode::DiskFilling);
    assert_eq!(disk(&with(86, 0, 10), &none), Some(Severity::Warn));
    assert_eq!(disk(&with(84, 0, 10), &none), None);
    assert_eq!(disk(&with(84, 0, 10), &is_open), Some(Severity::Warn));
    assert_eq!(disk(&with(93, 0, 10), &none), Some(Severity::Critical));
    assert_eq!(disk(&with(10, 9, 10), &none), Some(Severity::Warn));
    assert_eq!(disk(&with(10, 0, 0), &none), None);
}

#[test]
fn log_unbounded_uses_known_facts_when_omitted() {
    let mut r = base();
    let mut facts = r.workloads[APP].facts.take().unwrap();
    facts.log_max_size = None;
    let known = BTreeMap::from([(key(APP), facts)]);
    let ctx = Ctx::default();
    let input = JudgeInput {
        report: &r,
        facts: &known,
        recent: Recent {
            oom_events: &ctx.oom,
            exceptions: &ctx.exceptions,
        },
        open: &ctx.open,
    };
    assert!(
        judge(&input)
            .iter()
            .any(|f| f.code == IncidentCode::LogUnbounded)
    );
    // the postgres workload uses the `local` driver without max-size: rotated by default
    assert!(
        !judge(&input)
            .iter()
            .any(|f| f.subject == Subject::Workload(key(PG)))
    );
}

#[test]
fn datastore_reachability_and_replication_lag() {
    let with = |reachable, lag| {
        let mut r = base();
        r.workloads[PG].state.datastore = Some(DatastoreProbe {
            reachable,
            detail: "x".into(),
            replication_lag_s: Some(lag),
        });
        r
    };
    let none = Ctx::default();
    let is_open = open(Subject::Workload(key(PG)), IncidentCode::ReplicationLag);
    let lag = |r: &Report, ctx: &Ctx| severity_of(r, ctx, IncidentCode::ReplicationLag);
    let down = severity_of(&with(false, 0.0), &none, IncidentCode::DatastoreUnreachable);
    assert_eq!(down, Some(Severity::Critical));
    assert_eq!(lag(&with(true, 35.0), &none), Some(Severity::Warn));
    assert_eq!(lag(&with(true, 15.0), &none), None);
    assert_eq!(lag(&with(true, 15.0), &is_open), Some(Severity::Warn));
    assert_eq!(lag(&with(true, 400.0), &none), Some(Severity::Critical));
}

#[test]
fn app_exceptions_thresholds() {
    let r = base();
    let group = |class, component: &str, count, final_count| ExceptionGroup {
        workload: key(APP),
        class,
        component: component.into(),
        code: "X".into(),
        count,
        final_count,
        first_seen: ago(&r, 10),
        last_seen: ago(&r, 1),
        biz_keys: vec![],
        sample: None,
    };
    let check = |groups: Vec<ExceptionGroup>| {
        severity_of(
            &r,
            &Ctx {
                exceptions: groups,
                ..Ctx::default()
            },
            IncidentCode::AppExceptions,
        )
    };
    let app = ExceptionClass::Application;
    let split = vec![group(app, "a", 3, 3), group(app, "b", 2, 2)];
    assert_eq!(check(split), Some(Severity::Warn));
    assert_eq!(check(vec![group(app, "a", 4, 4)]), None);
    assert_eq!(check(vec![group(app, "a", 50, 0)]), Some(Severity::Warn));
    assert_eq!(
        check(vec![group(app, "_stderr", 10, 10)]),
        Some(Severity::Warn)
    );
    assert_eq!(
        check(vec![group(app, "a", 20, 20)]),
        Some(Severity::Critical)
    );
    assert_eq!(
        check(vec![group(ExceptionClass::Business, "a", 99, 99)]),
        None
    );
    assert_eq!(check(vec![group(app, "_skym", 99, 99)]), None);
}

#[test]
fn unhealthy_workload() {
    let mut r = base();
    r.workloads[APP].state.health = Some(Health::Unhealthy);
    let found = severity_of(&r, &Ctx::default(), IncidentCode::WorkloadUnhealthy);
    assert_eq!(found, Some(Severity::Critical));
}

#[test]
fn rollup_ignores_muted_and_takes_the_worst() {
    let view = |severity, muted| IncidentView {
        subject: Subject::Host("x".into()),
        code: IncidentCode::DiskFilling,
        severity,
        detail: String::new(),
        opened_at: None,
        open_for: None,
        resolved_at: None,
        muted,
        mute_reason: None,
        links: BTreeMap::new(),
    };
    assert_eq!(rollup(&[]), Status::Ok);
    assert_eq!(rollup(&[view(Severity::Critical, true)]), Status::Ok);
    let mixed = [view(Severity::Warn, false), view(Severity::Critical, false)];
    assert_eq!(rollup(&mixed), Status::Critical);
}
