use super::*;
use jiff::SignedDuration;
use skym_core::rules::rule;

fn at(min: i64) -> Timestamp {
    Timestamp::from_second(1_790_000_000 + min * 60).unwrap()
}

fn finding(code: IncidentCode, severity: Severity) -> Finding {
    Finding { subject: "workload:x/app/api".parse().unwrap(), code, severity, detail: "d".into() }
}

/// Feeds a sequence of matches (`Some`) and misses (`None`) one minute apart.
fn run(code: IncidentCode, steps: &[Option<Severity>]) -> (Option<Incident>, Vec<Change>) {
    let mut active: Option<Incident> = None;
    let mut resolved: Option<Incident> = None;
    let mut changes = Vec::new();
    for (i, step) in steps.iter().enumerate() {
        let f = step.map(|s| finding(code, s));
        let t =
            next(rule(code), active.as_ref(), resolved.as_ref(), f.as_ref(), None, at(i as i64));
        if let Some((mut row, change)) = t.write {
            row.id = row.id.or(Some(i as i64 + 1));
            changes.extend(change);
            if row.state == State::Resolved {
                resolved = Some(row);
                active = None;
            } else {
                active = Some(row);
            }
        } else if t.delete.is_some() {
            active = None;
        }
    }
    (active, changes)
}

const W: Option<Severity> = Some(Severity::Warn);
const C: Option<Severity> = Some(Severity::Critical);

#[test]
fn opens_after_consecutive_matches_and_a_single_blip_is_dropped() {
    let (row, changes) = run(IncidentCode::WorkloadDown, &[C, None]);
    assert!(row.is_none() && changes.is_empty(), "one match, then gone");
    let (row, changes) = run(IncidentCode::WorkloadDown, &[C, C]);
    assert_eq!((row.unwrap().state, changes), (State::Open, vec![Change::Opened]));
    let (row, _) = run(IncidentCode::DiskFilling, &[W]);
    assert_eq!(row.unwrap().opened_at, Some(at(0)), "open_after = 1 opens at once");
}

#[test]
fn resolves_after_consecutive_misses() {
    let (row, changes) = run(IncidentCode::WorkloadDown, &[C, C, None, C, None, None]);
    assert!(row.is_none());
    assert_eq!(changes, [Change::Opened, Change::Resolved]);
    let (row, _) = run(IncidentCode::WorkloadDown, &[C, C, None]);
    assert_eq!(row.unwrap().clear_streak, 1, "one miss is not enough");
}

#[test]
fn reopening_needs_the_same_debounce_and_the_window() {
    let blip = run(IncidentCode::WorkloadDown, &[C, C, None, None, C]).1;
    assert_eq!(blip, [Change::Opened, Change::Resolved], "one match does not reopen");
    let back = run(IncidentCode::WorkloadDown, &[C, C, None, None, C, C]).1;
    assert_eq!(back, [Change::Opened, Change::Resolved, Change::Reopened]);
    let mut late = vec![C, C, None, None];
    late.extend(std::iter::repeat_n(None, 40));
    late.extend([C, C]);
    let changes = run(IncidentCode::WorkloadDown, &late).1;
    assert_eq!(changes.last(), Some(&Change::Opened), "after 30 minutes it is a new incident");
}

#[test]
fn severity_follows_findings_keeps_its_peak_and_decays() {
    let (row, changes) = run(IncidentCode::CrashLoop, &[W, C, W]);
    let row = row.unwrap();
    assert_eq!((row.severity, row.peak_severity), (Severity::Warn, Severity::Critical));
    assert_eq!(changes, [Change::Opened, Change::Severity, Change::Severity]);

    let code = IncidentCode::WorkloadUnhealthy;
    let opened = Incident { opened_at: Some(at(0)), ..run(code, &[C, C]).0.unwrap() };
    let day = SignedDuration::from_hours(24);
    let before = next(
        rule(code),
        Some(&opened),
        None,
        Some(&finding(code, Severity::Critical)),
        None,
        at(0) + day - SignedDuration::from_secs(1),
    );
    assert_eq!(before.write.unwrap().0.severity, Severity::Critical);
    let after = next(
        rule(code),
        Some(&opened),
        None,
        Some(&finding(code, Severity::Critical)),
        None,
        at(0) + day,
    );
    assert_eq!(
        after.write.unwrap(),
        (
            Incident { severity: Severity::Warn, last_seen: at(0) + day, ..opened },
            Some(Change::Severity)
        )
    );
}

#[test]
fn reopening_drops_the_pending_row_it_replaces() {
    let code = IncidentCode::DiskFilling;
    let resolved = Incident {
        id: Some(7),
        state: State::Resolved,
        resolved_at: Some(at(0)),
        ..run(code, &[W]).0.unwrap()
    };
    let pending = Incident { id: Some(9), state: State::Pending, ..resolved.clone() };
    let f = finding(code, Severity::Critical);
    let t = next(
        Rule { open_after: 2, ..rule(code) },
        Some(&pending),
        Some(&resolved),
        Some(&f),
        None,
        at(5),
    );
    let (row, change) = t.write.unwrap();
    assert_eq!(
        (row.id, row.state, row.resolved_at, change),
        (Some(7), State::Open, None, Some(Change::Reopened))
    );
    assert_eq!(row.peak_severity, Severity::Critical);
    assert_eq!(t.delete, Some(9));
}

#[test]
fn every_update_carries_the_latest_finding_and_time() {
    let code = IncidentCode::WorkloadDown;
    let with =
        |detail: &str, severity| Finding { detail: detail.into(), ..finding(code, severity) };
    let pending =
        next(rule(code), None, None, Some(&with("exited (1)", Severity::Warn)), None, at(0))
            .write
            .unwrap()
            .0;
    let pending = Incident { id: Some(1), ..pending };
    let second = next(
        rule(code),
        Some(&pending),
        None,
        Some(&with("exited (2)", Severity::Critical)),
        None,
        at(1),
    );
    let open = second.write.unwrap().0;
    assert_eq!(
        (open.state, open.detail.as_str(), open.last_seen, open.severity),
        (State::Open, "exited (2)", at(1), Severity::Critical)
    );

    let missed = next(rule(code), Some(&open), None, None, None, at(2)).write.unwrap().0;
    assert_eq!(
        (missed.clear_streak, missed.last_seen),
        (1, at(1)),
        "a miss does not move last_seen"
    );
    let back = next(
        rule(code),
        Some(&missed),
        None,
        Some(&with("exited (3)", Severity::Critical)),
        None,
        at(3),
    );
    let back = back.write.unwrap().0;
    assert_eq!((back.clear_streak, back.detail.as_str(), back.last_seen), (0, "exited (3)", at(3)));

    let resolved =
        Incident { state: State::Resolved, resolved_at: Some(at(4)), clear_streak: 2, ..back };
    let again = Incident { id: Some(2), ..pending };
    let reopened = next(
        rule(code),
        Some(&again),
        Some(&resolved),
        Some(&with("exited (4)", Severity::Warn)),
        None,
        at(5),
    );
    let row = reopened.write.unwrap().0;
    assert_eq!(
        (row.clear_streak, row.detail.as_str(), row.last_seen, row.severity, row.peak_severity),
        (0, "exited (4)", at(5), Severity::Warn, Severity::Critical)
    );
}

#[test]
fn the_peak_counts_matches_before_opening() {
    let (row, _) = run(IncidentCode::WorkloadDown, &[W, C, W]);
    assert_eq!(row.unwrap().peak_severity, Severity::Critical);
}

#[test]
fn a_reopened_incident_keeps_its_decay() {
    let code = IncidentCode::WorkloadUnhealthy;
    let opened = run(code, &[C, C]).0.unwrap();
    let day = SignedDuration::from_hours(25);
    let resolved =
        Incident { id: Some(3), state: State::Resolved, resolved_at: Some(at(0) + day), ..opened };
    let f = finding(code, Severity::Critical);
    let pending = Incident { id: Some(4), state: State::Pending, ..resolved.clone() };
    let t = next(rule(code), Some(&pending), Some(&resolved), Some(&f), None, at(1) + day);
    assert_eq!(t.write.unwrap().0.severity, Severity::Warn);
}

#[test]
fn retiring_resolves_open_and_drops_pending() {
    let open = Incident { id: Some(5), ..run(IncidentCode::DiskFilling, &[W]).0.unwrap() };
    let t = retire(&open, at(9));
    let (row, change) = t.write.unwrap();
    assert_eq!(
        (row.state, row.resolved_at, change),
        (State::Resolved, Some(at(9)), Some(Change::Resolved))
    );
    let pending = Incident { state: State::Pending, ..open.clone() };
    assert_eq!(retire(&pending, at(9)), Transition { write: None, delete: Some(5) });
    let done = Incident { state: State::Resolved, ..open };
    assert_eq!(retire(&done, at(9)), Transition::default());
}

#[test]
fn states_and_changes_round_trip_and_reject_unknown_names() {
    for s in [State::Pending, State::Open, State::Resolved] {
        assert_eq!(s.as_str().parse::<State>(), Ok(s));
    }
    for c in [Change::Opened, Change::Reopened, Change::Resolved, Change::Severity] {
        assert_eq!(c.as_str().parse::<Change>(), Ok(c));
    }
    assert!("closed".parse::<State>().is_err() && "moved".parse::<Change>().is_err());
}
