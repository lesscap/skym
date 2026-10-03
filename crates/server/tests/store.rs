//! Ingest, heartbeat and maintenance at chosen times, against an in-memory database.

use jiff::{SignedDuration, Timestamp};
use rusqlite::Connection;
use skym_core::model::{ExceptionClass, ExceptionGroup, LocalEvent, RunState};
use skym_core::report::Report;
use skym_core::rules::IncidentCode;
use skym_server::evaluate::heartbeat_once;
use skym_server::ingest::{IngestError, Outcome, ingest};
use skym_server::lifecycle::{Change, State};
use skym_server::store::{history, hosts, incidents};
use skym_server::{db, tasks};

fn t0() -> Timestamp {
    "2026-10-01T12:00:00Z".parse().unwrap()
}

fn mins(m: i64) -> SignedDuration {
    SignedDuration::from_mins(m)
}

fn report(ts: Timestamp) -> Report {
    let mut r: Report = skym_core::fixtures::full_report();
    r.ts = ts;
    r.exceptions.clear();
    r
}

fn send(c: &mut Connection, r: &Report, now: Timestamp) -> Result<Outcome, IngestError> {
    ingest(c, "x", &serde_json::to_vec(r).unwrap(), now)
}

fn open_codes(c: &Connection) -> Vec<(String, IncidentCode)> {
    let mut found: Vec<_> = incidents::active_for_host(c, "x")
        .unwrap()
        .into_iter()
        .filter(|i| i.state == State::Open)
        .map(|i| (i.subject.to_string(), i.code))
        .collect();
    found.sort();
    found
}

fn beat(c: &mut Connection, now: Timestamp) -> Vec<IncidentCode> {
    heartbeat_once(c, &["x".into()], t0() - mins(60), SignedDuration::from_secs(60), now).unwrap();
    incidents::active_with_code(c, IncidentCode::HeartbeatLost)
        .unwrap()
        .into_iter()
        .map(|i| i.code)
        .collect()
}

#[test]
fn a_replayed_report_still_proves_the_host_alive() {
    let mut c = db::open_in_memory().unwrap();
    assert_eq!(send(&mut c, &report(t0()), t0()).unwrap(), Outcome::Accepted);
    assert_eq!(send(&mut c, &report(t0()), t0() + mins(10)).unwrap(), Outcome::Duplicate);
    assert!(beat(&mut c, t0() + mins(12)).is_empty(), "seen 2 minutes ago");
    assert_eq!(beat(&mut c, t0() + mins(14)), [IncidentCode::HeartbeatLost]);
}

#[test]
fn a_host_is_first_seen_with_its_first_report() {
    let mut c = db::open_in_memory().unwrap();
    send(&mut c, &report(t0()), t0()).unwrap();
    send(&mut c, &report(t0() + mins(1)), t0() + mins(1)).unwrap();
    let row = hosts::get(&c, "x").unwrap().unwrap();
    assert_eq!((row.first_seen, row.last_seen), (t0(), t0() + mins(1)));
}

#[test]
fn reports_may_run_ten_minutes_ahead_and_no_more() {
    let mut c = db::open_in_memory().unwrap();
    assert!(send(&mut c, &report(t0() + mins(10)), t0()).is_ok());
    let ahead =
        send(&mut c, &report(t0() + mins(10) + SignedDuration::from_secs(1)), t0() - mins(1));
    assert!(matches!(ahead, Err(IngestError::Invalid(_))));
    let week = mins(60 * 24 * 7);
    let ancient = send(&mut db::open_in_memory().unwrap(), &report(t0() - week - mins(1)), t0());
    assert!(matches!(ancient, Err(IngestError::Invalid(_))), "older than a week");
    assert!(send(&mut db::open_in_memory().unwrap(), &report(t0() - week), t0()).is_ok());
}

#[test]
fn oom_kills_count_for_an_hour() {
    let mut c = db::open_in_memory().unwrap();
    let mut r = report(t0());
    r.local_events = vec![LocalEvent::OomKilled { ts: t0() - mins(90), workload: None }];
    send(&mut c, &r, t0()).unwrap();
    assert!(open_codes(&c).is_empty(), "90 minutes ago is outside the window");
    let mut r = report(t0() + mins(1));
    r.local_events = vec![LocalEvent::OomKilled { ts: t0() - mins(30), workload: None }];
    send(&mut c, &r, t0() + mins(1)).unwrap();
    assert_eq!(open_codes(&c), [("host:x".to_string(), IncidentCode::OomKilled)]);
}

fn failures(r: &Report, final_count: u32) -> ExceptionGroup {
    ExceptionGroup {
        workload: r.workloads[0].key.clone(),
        class: ExceptionClass::Application,
        component: "jobs".into(),
        code: "TIMEOUT".into(),
        count: final_count,
        final_count,
        first_seen: r.ts,
        last_seen: r.ts,
        biz_keys: vec![],
        sample: None,
    }
}

#[test]
fn exceptions_add_up_over_fifteen_minutes() {
    for (gap, alarmed) in [(20, false), (10, true)] {
        let mut c = db::open_in_memory().unwrap();
        let mut early = report(t0() - mins(gap));
        early.exceptions = vec![failures(&early, 3)];
        send(&mut c, &early, t0() - mins(gap)).unwrap();
        let mut late = report(t0());
        late.exceptions = vec![failures(&late, 3)];
        send(&mut c, &late, t0()).unwrap();
        let open = open_codes(&c).iter().any(|(_, code)| *code == IncidentCode::AppExceptions);
        assert_eq!(open, alarmed, "3 + 3 failures {gap} minutes apart");
    }
}

#[test]
fn disk_growth_is_projected_from_reported_usage() {
    let mut c = db::open_in_memory().unwrap();
    for i in 0..37 {
        let mut r = report(t0() + mins(10 * i));
        let m = &mut r.host_state.mounts[0];
        (m.total_bytes, m.used_bytes) =
            (100_000_000_000, 10_000_000_000 + 10_000_000 * 60 * 10 * i as u64);
        send(&mut c, &r, r.ts).unwrap();
    }
    let disk = incidents::active_for_host(&c, "x")
        .unwrap()
        .into_iter()
        .find(|i| i.code == IncidentCode::DiskFilling);
    let disk = disk.expect("10 MB/s fills the rest within a day");
    assert!(disk.detail.contains("full in"), "{}", disk.detail);
}

fn down(mut r: Report, i: usize) -> Report {
    r.workloads[i].state.run = RunState::Exited;
    r.workloads[i].state.exit_code = Some(1);
    r
}

#[test]
fn a_failed_source_hides_only_what_it_did_not_report() {
    let mut c = db::open_in_memory().unwrap();
    for m in [0, 1] {
        send(&mut c, &down(down(report(t0() + mins(m)), 0), 1), t0() + mins(m)).unwrap();
    }
    assert_eq!(open_codes(&c).len(), 2);
    for m in [2, 3] {
        let mut partial = report(t0() + mins(m));
        partial.workloads.truncate(1); // the first workload, running again
        partial.errors = vec!["docker: inspect timed out".into()];
        send(&mut c, &partial, t0() + mins(m)).unwrap();
    }
    assert_eq!(open_codes(&c), [("workload:x/pg/main".to_string(), IncidentCode::WorkloadDown)]);
}

#[test]
fn maintenance_archives_gone_workloads_and_applies_retention() {
    let mut c = db::open_in_memory().unwrap();
    let mut r = down(report(t0()), 0);
    r.local_events = vec![LocalEvent::OomKilled { ts: t0(), workload: None }];
    for m in [0, 1] {
        r.ts = t0() + mins(m);
        send(&mut c, &r, r.ts).unwrap();
    }
    tasks::maintain(&mut c, t0() + mins(60 * 24 * 6)).unwrap();
    assert_eq!(hosts::workloads(&c, "x").unwrap().len(), 3, "six days: kept");
    tasks::maintain(&mut c, t0() + mins(60 * 24 * 8)).unwrap();
    assert!(hosts::workloads(&c, "x").unwrap().is_empty(), "eight days: archived");
    assert!(
        open_codes(&c).iter().all(|(s, _)| !s.starts_with("workload:")),
        "their incidents closed"
    );
    let closed = incidents::changes(&c, "x", None, Timestamp::UNIX_EPOCH, 10).unwrap();
    assert_eq!(closed[0].change, Change::Resolved, "and the timeline says so");
    assert!(
        hosts::disk_samples(&c, "x", "/", Timestamp::UNIX_EPOCH).unwrap().is_empty(),
        "samples kept 7 days"
    );
    assert!(
        !history::events(&c, "x", None, Timestamp::UNIX_EPOCH, 10).unwrap().is_empty(),
        "events kept 90 days"
    );
    // Resolved on day 8 by archiving; 90 days later it goes.
    tasks::maintain(&mut c, t0() + mins(60 * 24 * 99)).unwrap();
    assert!(history::events(&c, "x", None, Timestamp::UNIX_EPOCH, 10).unwrap().is_empty());
    assert!(hosts::archived(&c).unwrap().is_empty(), "archived workloads kept 90 days");
    let resolved = incidents::listed(&c, false, None, None, Timestamp::UNIX_EPOCH, 10).unwrap();
    assert!(resolved.is_empty(), "resolved incidents kept 90 days");
}

fn disk(ts: Timestamp, percent: u64) -> Report {
    let mut r = report(ts);
    let m = &mut r.host_state.mounts[0];
    (m.total_bytes, m.used_bytes) = (100, percent);
    r
}

#[test]
fn an_open_disk_incident_holds_above_the_resolve_threshold() {
    let mut c = db::open_in_memory().unwrap();
    send(&mut c, &disk(t0(), 90), t0()).unwrap();
    send(&mut c, &disk(t0() + mins(1), 84), t0() + mins(1)).unwrap();
    assert_eq!(
        open_codes(&c),
        [("mount:x:/".to_string(), IncidentCode::DiskFilling)],
        "84% is above 82%"
    );
    let mut partial = disk(t0() + mins(2), 50);
    partial.errors = vec!["docker: unavailable".into()];
    send(&mut c, &partial, t0() + mins(2)).unwrap();
    assert!(open_codes(&c).is_empty(), "a reported mount resolves despite another source failing");
}

#[test]
fn a_recurrence_within_half_an_hour_reopens_the_same_incident() {
    let mut c = db::open_in_memory().unwrap();
    let at = |m| t0() + mins(m);
    for (m, percent) in [(0, 90), (1, 50), (10, 90)] {
        send(&mut c, &disk(at(m), percent), at(m)).unwrap();
    }
    assert_eq!(
        incidents::listed(&c, true, None, None, Timestamp::UNIX_EPOCH, 10).unwrap().len(),
        1
    );
    assert!(
        incidents::listed(&c, false, None, None, Timestamp::UNIX_EPOCH, 10).unwrap().is_empty(),
        "no second row"
    );
    let changes: Vec<Change> = incidents::changes(&c, "x", None, Timestamp::UNIX_EPOCH, 10)
        .unwrap()
        .into_iter()
        .map(|l| l.change)
        .collect();
    assert_eq!(changes, [Change::Reopened, Change::Resolved, Change::Opened], "newest first");
}

#[test]
fn a_server_restart_neither_confirms_nor_clears_a_lost_heartbeat() {
    let mut c = db::open_in_memory().unwrap();
    send(&mut c, &report(t0()), t0()).unwrap();
    assert_eq!(beat(&mut c, t0() + mins(10)), [IncidentCode::HeartbeatLost]);
    let restarted = t0() + mins(20);
    let after = |c: &mut Connection, now| {
        heartbeat_once(c, &["x".into()], restarted, SignedDuration::from_secs(60), now).unwrap();
        incidents::active_with_code(c, IncidentCode::HeartbeatLost).unwrap().len()
    };
    assert_eq!(after(&mut c, restarted + mins(1)), 1, "still open during the grace period");
    assert_eq!(after(&mut c, restarted + mins(4)), 1, "and after it");
    let log = incidents::changes(&c, "x", None, Timestamp::UNIX_EPOCH, 10).unwrap();
    assert_eq!(log.len(), 1, "no false resolve and reopen");
}

#[test]
fn a_failed_source_still_reports_new_problems() {
    let mut c = db::open_in_memory().unwrap();
    let mut r = report(t0());
    let key = r.workloads[0].key.clone();
    r.workloads.clear();
    r.errors = vec!["docker: unavailable".into()];
    r.local_events = vec![LocalEvent::OomKilled { ts: t0(), workload: Some(key) }];
    send(&mut c, &r, t0()).unwrap();
    assert_eq!(open_codes(&c), [("workload:x/captain/api".to_string(), IncidentCode::OomKilled)]);
}

#[test]
fn a_class_from_a_newer_agent_does_not_block_the_host() {
    let mut c = db::open_in_memory().unwrap();
    let mut r = report(t0());
    let mut group = failures(&r, 1);
    group.class = ExceptionClass::Unknown;
    r.exceptions = vec![group];
    assert_eq!(send(&mut c, &r, t0()).unwrap(), Outcome::Accepted);
    assert_eq!(send(&mut c, &report(t0() + mins(1)), t0() + mins(1)).unwrap(), Outcome::Accepted);
    let window = (t0() - mins(5), t0() + mins(5));
    assert_eq!(
        history::exceptions(&c, "x", window, None, None).unwrap()[0].class,
        ExceptionClass::Unknown
    );
}

#[test]
fn a_failed_source_does_not_clear_unbounded_logs() {
    let mut c = db::open_in_memory().unwrap();
    let unbounded = |ts: Timestamp| {
        let mut r = report(ts);
        r.workloads[0].facts.as_mut().unwrap().log_max_size = None;
        r
    };
    send(&mut c, &unbounded(t0()), t0()).unwrap();
    let host_logs = ("host:x".to_string(), IncidentCode::LogUnbounded);
    assert!(open_codes(&c).contains(&host_logs));
    let mut partial = report(t0() + mins(1));
    partial.workloads.clear();
    partial.errors = vec!["docker: connection refused".into()];
    send(&mut c, &partial, t0() + mins(1)).unwrap();
    assert!(open_codes(&c).contains(&host_logs), "unknown, not recovered");
    send(&mut c, &report(t0() + mins(2)), t0() + mins(2)).unwrap();
    assert!(!open_codes(&c).contains(&host_logs), "every container bounded again");
}
