//! Judgement rules: each test starts from a healthy report and breaks one thing.

use skym_core::judge::{JudgeInput, Recent, judge};
use skym_core::model::{
    DatastoreProbe, ExceptionClass, ExceptionGroup, Health, LocalEvent, RunState,
};
use skym_core::report::Report;
use skym_core::rules::{Finding, IncidentCode, Severity};
use skym_core::subject::{Subject, WorkloadKey};
use skym_core::time::{SignedDuration, Timestamp};
use skym_core::view::{IncidentView, Status, rollup, workload_summary};
use std::collections::{BTreeMap, BTreeSet};

const APP: usize = 0;
const PG: usize = 1;
const XRAY: usize = 2;

fn base() -> Report {
    let path = format!("{}/tests/fixtures/report-full.json", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[derive(Default)]
struct Ctx {
    oom: Vec<LocalEvent>,
    exceptions: Vec<ExceptionGroup>,
    open: BTreeSet<(Subject, IncidentCode)>,
}

fn findings(report: &Report, ctx: &Ctx) -> Vec<Finding> {
    let facts = BTreeMap::new();
    let input = JudgeInput {
        report,
        facts: &facts,
        recent: Recent { oom_events: &ctx.oom, exceptions: &ctx.exceptions },
        open: &ctx.open,
    };
    judge(&input)
}

fn run(report: &Report, ctx: &Ctx) -> Vec<(Subject, IncidentCode, Severity)> {
    findings(report, ctx).into_iter().map(|f| (f.subject, f.code, f.severity)).collect()
}

fn severity_of(report: &Report, ctx: &Ctx, code: IncidentCode) -> Option<Severity> {
    run(report, ctx).into_iter().find(|(_, c, _)| *c == code).map(|(_, _, s)| s)
}

fn key(i: usize) -> WorkloadKey {
    base().workloads[i].key.clone()
}

fn ago(report: &Report, mins: i64) -> Timestamp {
    report.ts - SignedDuration::from_mins(mins)
}

fn open(subject: Subject, code: IncidentCode) -> Ctx {
    Ctx { open: BTreeSet::from([(subject, code)]), ..Ctx::default() }
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
    assert_eq!(check(RunState::Exited, Some(1), vec![], APP), Some(Severity::Critical));
    assert_eq!(check(RunState::Restarting, None, vec![], APP), Some(Severity::Critical));
    assert_eq!(check(RunState::Exited, Some(0), vec![], APP), None);
    assert_eq!(check(RunState::Unknown, None, vec![], APP), None);
    assert_eq!(check(RunState::Running, None, vec![443], XRAY), Some(Severity::Critical));
}

#[test]
fn a_stopped_workload_says_since_when_and_why_it_stays_down() {
    let detail = |r: &Report| {
        findings(r, &Ctx::default())
            .into_iter()
            .find(|f| f.code == IncidentCode::WorkloadDown)
            .map(|f| f.detail)
    };
    let mut r = base();
    let w = &mut r.workloads[APP];
    (w.state.run, w.state.exit_code) = (RunState::Exited, Some(137));
    assert_eq!(detail(&r).as_deref(), Some("exited (137)"));
    let three_days_ago = ago(&r, 3 * 24 * 60);
    let w = &mut r.workloads[APP];
    w.state.state_since = Some(three_days_ago);
    w.state.oom_killed = true;
    w.facts.as_mut().unwrap().restart_policy = None; // the agent reports "no" as none
    assert_eq!(detail(&r).as_deref(), Some("exited (137), for 3d, OOM killed, no restart policy"));
    let mut unit = base();
    unit.workloads[XRAY].state.run = RunState::Dead;
    assert_eq!(detail(&unit).as_deref(), Some("dead"), "systemd restarts by its own unit file");
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
    let cases: [(&[i64], &Ctx, Option<Severity>); 7] = [
        (&[1, 2], &none, None),
        (&[1, 2, 60], &none, None), // exactly one hour ago is outside the window
        (&[1, 2, 59], &none, Some(Severity::Warn)),
        (&[1; 9], &none, Some(Severity::Warn)),
        (&[1; 10], &none, Some(Severity::Critical)),
        (&[29], &is_open, Some(Severity::Warn)),
        (&[30], &is_open, None),
    ];
    for (mins, ctx, expected) in cases {
        assert_eq!(
            severity_of(&with_restarts(mins), ctx, IncidentCode::CrashLoop),
            expected,
            "{mins:?}"
        );
    }
}

#[test]
fn oom_kills_are_grouped_per_subject() {
    let r = base();
    let event = |workload| LocalEvent::OomKilled { ts: ago(&r, 5), workload };
    let ctx = Ctx {
        oom: vec![
            event(Some(key(APP))),
            event(Some(key(APP))),
            event(Some(key(APP))),
            event(Some(key(PG))),
            event(Some(key(PG))),
            event(None),
            LocalEvent::Unknown,
        ],
        ..Ctx::default()
    };
    let found = run(&r, &ctx);
    assert!(found.contains(&(
        Subject::Workload(key(APP)),
        IncidentCode::OomKilled,
        Severity::Critical
    )));
    assert!(found.contains(&(Subject::Workload(key(PG)), IncidentCode::OomKilled, Severity::Warn)));
    assert!(found.contains(&(Subject::Host("x".into()), IncidentCode::OomKilled, Severity::Warn)));
    assert_eq!(found.len(), 3);
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
    let mount = Subject::Mount { host: "x".into(), path: "/".into() };
    let is_open = open(mount, IncidentCode::DiskFilling);
    let disk = |r: &Report, ctx: &Ctx| severity_of(r, ctx, IncidentCode::DiskFilling);
    let cases = [
        (with(84, 0, 10), &none, None),
        (with(85, 0, 10), &none, Some(Severity::Warn)),
        (with(91, 0, 10), &none, Some(Severity::Warn)),
        (with(92, 0, 10), &none, Some(Severity::Critical)),
        (with(82, 0, 10), &is_open, Some(Severity::Warn)),
        (with(81, 0, 10), &is_open, None),
        (with(10, 9, 10), &none, Some(Severity::Warn)), // inodes alone
        (with(10, 0, 0), &none, None),
        (with(10, 5, 0), &none, None), // no inode accounting
    ];
    for (i, (report, ctx, expected)) in cases.iter().enumerate() {
        assert_eq!(disk(report, ctx), *expected, "case {i}");
    }
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
        recent: Recent { oom_events: &ctx.oom, exceptions: &ctx.exceptions },
        open: &ctx.open,
    };
    // the postgres workload uses the `local` driver without max-size: rotated by default
    let found: Vec<_> =
        judge(&input).into_iter().filter(|f| f.code == IncidentCode::LogUnbounded).collect();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].subject, Subject::Host(r.host.clone()));
    assert_eq!(
        found[0].detail,
        format!(
            "1 container logs to json-file without max-size: {}/{}",
            key(APP).project,
            key(APP).service
        )
    );
}

#[test]
fn log_unbounded_is_one_finding_per_host_naming_five() {
    let mut r = base();
    let template = r.workloads[APP].clone();
    r.workloads = (1..=7)
        .map(|i| {
            let mut w = template.clone();
            w.key.service = format!("s{i}");
            if i == 1 {
                w.key.project = "-".into(); // a container outside compose
            }
            w.facts.as_mut().unwrap().log_max_size = None;
            w
        })
        .collect();
    let found = severity_of(&r, &Ctx::default(), IncidentCode::LogUnbounded);
    assert_eq!(found, Some(Severity::Info), "hygiene, not a reason to call the host unwell");
    let detail = findings(&r, &Ctx::default())
        .into_iter()
        .find(|f| f.code == IncidentCode::LogUnbounded)
        .unwrap()
        .detail;
    let p = &template.key.project;
    assert_eq!(
        detail,
        format!(
            "7 containers log to json-file without max-size: s1, {p}/s2, {p}/s3, {p}/s4, {p}/s5 and 2 more"
        )
    );
    for w in &mut r.workloads {
        w.facts.as_mut().unwrap().log_max_size = Some("10m".into());
    }
    assert_eq!(severity_of(&r, &Ctx::default(), IncidentCode::LogUnbounded), None);
}

#[test]
fn datastore_reachability_and_replication_lag() {
    let with = |reachable, lag| {
        let mut r = base();
        r.workloads[PG].state.datastore =
            Some(DatastoreProbe { reachable, detail: "x".into(), replication_lag_s: Some(lag) });
        r
    };
    let none = Ctx::default();
    let is_open = open(Subject::Workload(key(PG)), IncidentCode::ReplicationLag);
    let lag = |r: &Report, ctx: &Ctx| severity_of(r, ctx, IncidentCode::ReplicationLag);
    let down = severity_of(&with(false, 0.0), &none, IncidentCode::DatastoreUnreachable);
    assert_eq!(down, Some(Severity::Critical));
    let cases = [
        (30.0, &none, None),
        (30.5, &none, Some(Severity::Warn)),
        (300.0, &none, Some(Severity::Warn)),
        (300.5, &none, Some(Severity::Critical)),
        (10.0, &is_open, Some(Severity::Warn)),
        (9.9, &is_open, None),
    ];
    for (seconds, ctx, expected) in cases {
        assert_eq!(lag(&with(true, seconds), ctx), expected, "{seconds}s");
    }
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
        severity_of(&r, &Ctx { exceptions: groups, ..Ctx::default() }, IncidentCode::AppExceptions)
    };
    let app = ExceptionClass::Application;
    let cases = [
        (vec![group(app, "a", 3, 3), group(app, "b", 2, 2)], Some(Severity::Warn)), // summed
        (vec![group(app, "a", 4, 4)], None),
        (vec![group(app, "a", 49, 0)], None),
        (vec![group(app, "a", 50, 0)], Some(Severity::Warn)),
        (vec![group(app, "_stderr", 9, 9)], None),
        (vec![group(app, "_stderr", 10, 10)], Some(Severity::Warn)),
        (vec![group(app, "a", 19, 19)], Some(Severity::Warn)),
        (vec![group(app, "a", 20, 20)], Some(Severity::Critical)),
        (vec![group(ExceptionClass::Business, "a", 99, 99)], None),
        (
            vec![ExceptionGroup { code: "_PROTOCOL_ERROR".into(), ..group(app, "_skym", 99, 99) }],
            None,
        ), // protocol errors never alarm
        (
            vec![ExceptionGroup { code: "_OVERFLOW".into(), ..group(app, "_skym", 5, 5) }],
            Some(Severity::Warn),
        ),
    ];
    for (i, (groups, expected)) in cases.into_iter().enumerate() {
        assert_eq!(check(groups), expected, "case {i}");
    }
}

#[test]
fn unhealthy_workload_while_running_with_what_the_check_said() {
    let unhealthy = |r: &Report| {
        findings(r, &Ctx::default()).into_iter().find(|f| f.code == IncidentCode::WorkloadUnhealthy)
    };
    let mut r = base();
    r.workloads[APP].state.health = Some(Health::Unhealthy);
    let found = unhealthy(&r).unwrap();
    assert_eq!(
        (found.severity, found.detail.as_str()),
        (Severity::Critical, "healthcheck failing")
    );
    let state = &mut r.workloads[APP].state;
    state.health_failing_streak = Some(3471);
    state.health_output = Some("exec: \"wget\": executable file not found".into());
    assert_eq!(
        unhealthy(&r).unwrap().detail,
        "failing 3471 checks: exec: \"wget\": executable file not found"
    );
    (r.workloads[APP].state.run, r.workloads[APP].state.exit_code) = (RunState::Exited, Some(1));
    assert!(unhealthy(&r).is_none(), "a stopped workload is down, not unhealthy");
}

fn incident(subject: Subject, severity: Severity, muted: bool) -> IncidentView {
    IncidentView {
        subject,
        code: IncidentCode::DiskFilling,
        severity,
        detail: String::new(),
        opened_at: None,
        since: None,
        open_for: None,
        resolved_at: None,
        muted,
        mute_reason: None,
        links: BTreeMap::new(),
    }
}

#[test]
fn only_a_stopped_workload_knows_since_when_it_is_down() {
    let mut state = base().workloads[APP].state.clone();
    state.state_since = Some(ago(&base(), 60));
    assert_eq!(state.stopped_since(), None, "running: the start time is not a problem's start");
    state.run = RunState::Exited;
    assert_eq!(state.stopped_since(), state.state_since);
    state.run = RunState::Unknown;
    assert_eq!(state.stopped_since(), None);
}

#[test]
fn statuses_order_by_urgency() {
    let mut all = [Status::Critical, Status::Ok, Status::Unknown, Status::Warn];
    all.sort();
    assert_eq!(all, [Status::Ok, Status::Warn, Status::Unknown, Status::Critical]);
    assert!(Status::Unknown > Status::Warn && Status::Unknown < Status::Critical);
}

#[test]
fn rollup_ignores_muted_and_takes_the_worst() {
    let view = |severity, muted| incident(Subject::Host("x".into()), severity, muted);
    assert_eq!(rollup(&[]), Status::Ok);
    assert_eq!(rollup(&[view(Severity::Critical, true)]), Status::Ok);
    let mixed = [view(Severity::Warn, false), view(Severity::Critical, false)];
    assert_eq!(rollup(&mixed), Status::Critical);
    assert_eq!(rollup(&[view(Severity::Info, false)]), Status::Ok, "hygiene only");
}

#[test]
fn a_workload_summary_counts_only_its_own_incidents() {
    let r = base();
    let key = |i: usize| r.workloads[i].key.clone();
    let incidents = [
        incident(Subject::Workload(key(APP)), Severity::Warn, false),
        incident(Subject::Workload(key(PG)), Severity::Critical, false),
        incident(Subject::Host(r.host.clone()), Severity::Critical, false),
    ];
    let summary = |i: usize| {
        let w = &r.workloads[i];
        workload_summary(w.key.clone(), w.facts.as_ref(), &w.state, &incidents, BTreeMap::new())
    };
    assert_eq!(summary(APP).status, Status::Warn);
    assert_eq!(summary(XRAY).status, Status::Ok);
}

#[test]
fn at_most_one_finding_per_subject_and_code() {
    let mut r = base();
    let recent = ago(&r, 1);
    for w in &mut r.workloads {
        w.state.run = RunState::Dead;
        w.state.health = Some(Health::Unhealthy);
        w.state.restarts = vec![recent; 20];
        w.state.missing_ports = vec![443];
        w.state.datastore = Some(DatastoreProbe {
            reachable: false,
            detail: "x".into(),
            replication_lag_s: Some(900.0),
        });
        if let Some(f) = w.facts.as_mut() {
            (f.log_driver, f.log_max_size) = (Some("json-file".into()), None);
        }
    }
    let m = &mut r.host_state.mounts[0];
    (m.used_bytes, m.inodes_used) = (m.total_bytes, m.inodes_total);
    let found = run(&r, &Ctx::default());
    let unique: BTreeSet<_> = found.iter().map(|(s, c, _)| (s.clone(), *c)).collect();
    assert_eq!(unique.len(), found.len());
    // 3 dead workloads × 4 rules (unhealthy needs a running one) + 1 mount + 1 host-wide
    assert_eq!(found.len(), 14);
}
