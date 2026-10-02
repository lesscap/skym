//! How one `(subject, code)` moves between pending, open and resolved. Pure: the store
//! loads the rows, `next` decides, the store writes the result.

use jiff::Timestamp;
use skym_core::rules::{Finding, IncidentCode, REOPEN_WINDOW, Rule, Severity};
use skym_core::subject::{HostId, Subject};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Pending,
    Open,
    Resolved,
}

impl State {
    pub const fn as_str(self) -> &'static str {
        match self {
            State::Pending => "pending",
            State::Open => "open",
            State::Resolved => "resolved",
        }
    }
}

impl std::str::FromStr for State {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        [State::Pending, State::Open, State::Resolved]
            .into_iter()
            .find(|v| v.as_str() == s)
            .ok_or_else(|| format!("unknown incident state {s}"))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Incident {
    /// `None` until stored.
    pub id: Option<i64>,
    pub host: Option<HostId>,
    pub subject: Subject,
    pub code: IncidentCode,
    pub state: State,
    pub severity: Severity,
    pub peak_severity: Severity,
    pub detail: String,
    pub match_streak: u32,
    pub clear_streak: u32,
    pub first_match_at: Timestamp,
    pub opened_at: Option<Timestamp>,
    pub last_seen: Timestamp,
    pub resolved_at: Option<Timestamp>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Opened,
    Reopened,
    Resolved,
    Severity,
}

impl Change {
    pub const fn as_str(self) -> &'static str {
        match self {
            Change::Opened => "opened",
            Change::Reopened => "reopened",
            Change::Resolved => "resolved",
            Change::Severity => "severity",
        }
    }
}

impl std::str::FromStr for Change {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        [Change::Opened, Change::Reopened, Change::Resolved, Change::Severity]
            .into_iter()
            .find(|v| v.as_str() == s)
            .ok_or_else(|| format!("unknown incident change {s}"))
    }
}

/// What to store: at most one row to write (with its log entry) and one pending row to drop.
#[derive(Debug, Default, PartialEq)]
pub struct Transition {
    pub write: Option<(Incident, Option<Change>)>,
    pub delete: Option<i64>,
}

/// `active` is the pending or open row; `reopenable` the most recent resolved row, if any.
pub fn next(
    rule: Rule,
    active: Option<&Incident>,
    reopenable: Option<&Incident>,
    finding: Option<&Finding>,
    host: Option<&HostId>,
    now: Timestamp,
) -> Transition {
    match (active, finding) {
        (None, None) => Transition::default(),
        (None, Some(f)) => matched_pending(rule, new_pending(f, host, now), reopenable, f, now),
        (Some(p), Some(f)) if p.state == State::Pending => {
            let p = Incident { match_streak: p.match_streak + 1, ..p.clone() };
            matched_pending(rule, p, reopenable, f, now)
        }
        (Some(p), None) if p.state == State::Pending => Transition { write: None, delete: p.id },
        (Some(open), Some(f)) => still_matching(rule, open, f, now),
        (Some(open), None) => clearing(rule, open, now),
    }
}

fn new_pending(f: &Finding, host: Option<&HostId>, now: Timestamp) -> Incident {
    Incident {
        id: None,
        host: host.cloned(),
        subject: f.subject.clone(),
        code: f.code,
        state: State::Pending,
        severity: f.severity,
        peak_severity: f.severity,
        detail: f.detail.clone(),
        match_streak: 1,
        clear_streak: 0,
        first_match_at: now,
        opened_at: None,
        last_seen: now,
        resolved_at: None,
    }
}

/// A pending row that matched again: it opens once it matched `open_after` times in a row,
/// reusing a row resolved within the reopen window.
fn matched_pending(
    rule: Rule,
    p: Incident,
    reopenable: Option<&Incident>,
    f: &Finding,
    now: Timestamp,
) -> Transition {
    let p = Incident {
        severity: f.severity,
        peak_severity: p.peak_severity.max(f.severity),
        detail: f.detail.clone(),
        last_seen: now,
        ..p
    };
    if p.match_streak < rule.open_after {
        return Transition { write: Some((p, None)), delete: None };
    }
    let recent = reopenable.filter(|r| r.resolved_at.is_some_and(|at| now - REOPEN_WINDOW <= at));
    match recent {
        Some(r) => {
            let severity = decayed(rule, r.opened_at, f.severity, now);
            let reopened = Incident {
                state: State::Open,
                severity,
                peak_severity: r.peak_severity.max(severity),
                detail: f.detail.clone(),
                clear_streak: 0,
                last_seen: now,
                resolved_at: None,
                ..r.clone()
            };
            Transition { write: Some((reopened, Some(Change::Reopened))), delete: p.id }
        }
        None => {
            let opened = Incident { state: State::Open, opened_at: Some(now), ..p };
            Transition { write: Some((opened, Some(Change::Opened))), delete: None }
        }
    }
}

/// Warn once the rule's decay time has passed since the incident opened.
fn decayed(
    rule: Rule,
    opened_at: Option<Timestamp>,
    severity: Severity,
    now: Timestamp,
) -> Severity {
    let old = rule
        .decay_to_warn_after
        .zip(opened_at)
        .is_some_and(|(after, opened)| now - after >= opened);
    if old { Severity::Warn } else { severity }
}

fn still_matching(rule: Rule, open: &Incident, f: &Finding, now: Timestamp) -> Transition {
    let severity = decayed(rule, open.opened_at, f.severity, now);
    let change = (severity != open.severity).then_some(Change::Severity);
    let row = Incident {
        severity,
        peak_severity: open.peak_severity.max(severity),
        detail: f.detail.clone(),
        clear_streak: 0,
        last_seen: now,
        ..open.clone()
    };
    Transition { write: Some((row, change)), delete: None }
}

fn clearing(rule: Rule, open: &Incident, now: Timestamp) -> Transition {
    let row = Incident { clear_streak: open.clear_streak + 1, ..open.clone() };
    let row = match row.clear_streak >= rule.resolve_after {
        true => Incident { state: State::Resolved, resolved_at: Some(now), ..row },
        false => row,
    };
    let change = (row.state == State::Resolved).then_some(Change::Resolved);
    Transition { write: Some((row, change)), delete: None }
}

/// The subject is gone (an archived workload): open incidents resolve, pending ones go.
pub fn retire(active: &Incident, now: Timestamp) -> Transition {
    match active.state {
        State::Pending => Transition { write: None, delete: active.id },
        State::Open => {
            let row = Incident { state: State::Resolved, resolved_at: Some(now), ..active.clone() };
            Transition { write: Some((row, Some(Change::Resolved))), delete: None }
        }
        State::Resolved => Transition::default(),
    }
}

#[cfg(test)]
#[path = "lifecycle_tests.rs"]
mod tests;
