//! Turns one report into findings. Pure: the caller supplies recent history and
//! which incidents are already open (to pick resolve thresholds).

use crate::model::{
    ExceptionClass, ExceptionGroup, Health, LocalEvent, MountState, RunState, WorkloadFacts,
    WorkloadState,
};
use crate::report::{Report, WorkloadReport};
use crate::rules::{Finding, IncidentCode, Severity};
use crate::subject::{Subject, WorkloadKey};
use crate::time::{SignedDuration, Timestamp};
use std::collections::{BTreeMap, BTreeSet};

/// History the report alone does not carry, already filtered by the caller.
pub struct Recent<'a> {
    /// OOM kills within the last hour.
    pub oom_events: &'a [LocalEvent],
    /// Exception groups within the last 15 minutes.
    pub exceptions: &'a [ExceptionGroup],
}

pub struct JudgeInput<'a> {
    /// Already rehosted. Its `ts` is the reference time for every window.
    pub report: &'a Report,
    /// Known facts for workloads whose report omits them.
    pub facts: &'a BTreeMap<WorkloadKey, WorkloadFacts>,
    pub recent: Recent<'a>,
    pub open: &'a BTreeSet<(Subject, IncidentCode)>,
}

type Hit = (IncidentCode, Severity, String);

/// At most one finding per `(subject, code)`.
pub fn judge(input: &JudgeInput) -> Vec<Finding> {
    let report = input.report;
    let workloads = report
        .workloads
        .iter()
        .flat_map(|w| workload_findings(input, w));
    let mounts = report
        .host_state
        .mounts
        .iter()
        .filter_map(|m| disk(input, m));
    workloads
        .chain(mounts)
        .chain(oom(input))
        .chain(app_exceptions(input))
        .collect()
}

fn workload_findings(input: &JudgeInput, w: &WorkloadReport) -> Vec<Finding> {
    let subject = Subject::Workload(w.key.clone());
    let facts = w.facts.as_ref().or_else(|| input.facts.get(&w.key));
    let is_open = |code| input.open.contains(&(subject.clone(), code));
    let now = input.report.ts;
    [
        down(&w.state),
        unhealthy(&w.state),
        crash_loop(&w.state, now, is_open(IncidentCode::CrashLoop)),
        log_unbounded(facts),
        datastore_unreachable(&w.state),
        replication_lag(&w.state, is_open(IncidentCode::ReplicationLag)),
    ]
    .into_iter()
    .flatten()
    .map(|(code, severity, detail)| Finding {
        subject: subject.clone(),
        code,
        severity,
        detail,
    })
    .collect()
}

fn down(state: &WorkloadState) -> Option<Hit> {
    let not_running = match state.run {
        RunState::Running | RunState::Unknown => None,
        RunState::Exited if state.exit_code == Some(0) => None,
        RunState::Exited => Some(format!("exited ({})", state.exit_code.unwrap_or(-1))),
        other => Some(format!("{other:?}").to_lowercase()),
    };
    let not_listening = || {
        let ports: Vec<String> = state.missing_ports.iter().map(u16::to_string).collect();
        (!ports.is_empty()).then(|| format!("not listening on {}", ports.join(", ")))
    };
    let detail = not_running.or_else(not_listening)?;
    Some((IncidentCode::WorkloadDown, Severity::Critical, detail))
}

fn unhealthy(state: &WorkloadState) -> Option<Hit> {
    (state.health == Some(Health::Unhealthy)).then(|| {
        (
            IncidentCode::WorkloadUnhealthy,
            Severity::Critical,
            "healthcheck unhealthy".to_string(),
        )
    })
}

fn crash_loop(state: &WorkloadState, now: Timestamp, open: bool) -> Option<Hit> {
    let since = |window| state.restarts.iter().filter(|t| **t > now - window).count();
    let last_hour = since(SignedDuration::from_hours(1));
    let matched = if open {
        since(SignedDuration::from_mins(30)) >= 1
    } else {
        last_hour >= 3
    };
    let severity = if last_hour >= 10 {
        Severity::Critical
    } else {
        Severity::Warn
    };
    matched.then(|| {
        (
            IncidentCode::CrashLoop,
            severity,
            format!("{last_hour} restarts in the last hour"),
        )
    })
}

fn log_unbounded(facts: Option<&WorkloadFacts>) -> Option<Hit> {
    let facts = facts?;
    (facts.log_driver.as_deref() == Some("json-file") && facts.log_max_size.is_none()).then(|| {
        let detail = "json-file log driver without max-size".to_string();
        (IncidentCode::LogUnbounded, Severity::Warn, detail)
    })
}

fn datastore_unreachable(state: &WorkloadState) -> Option<Hit> {
    let probe = state.datastore.as_ref().filter(|d| !d.reachable)?;
    Some((
        IncidentCode::DatastoreUnreachable,
        Severity::Critical,
        probe.detail.clone(),
    ))
}

fn replication_lag(state: &WorkloadState, open: bool) -> Option<Hit> {
    let lag = state.datastore.as_ref()?.replication_lag_s?;
    let matched = if open { lag >= 10.0 } else { lag > 30.0 };
    let severity = if lag > 300.0 {
        Severity::Critical
    } else {
        Severity::Warn
    };
    matched.then(|| {
        (
            IncidentCode::ReplicationLag,
            severity,
            format!("replication lag {lag:.0}s"),
        )
    })
}

fn disk(input: &JudgeInput, m: &MountState) -> Option<Finding> {
    let ratio = |used: u64, total: u64| (total > 0).then(|| used as f64 / total as f64);
    let (p, what) = [
        (ratio(m.used_bytes, m.total_bytes), "used"),
        (ratio(m.inodes_used, m.inodes_total), "inodes used"),
    ]
    .into_iter()
    .filter_map(|(r, what)| r.map(|r| (r, what)))
    .max_by(|a, b| a.0.total_cmp(&b.0))?;
    let subject = Subject::Mount {
        host: input.report.host.clone(),
        path: m.path.clone(),
    };
    let open = input
        .open
        .contains(&(subject.clone(), IncidentCode::DiskFilling));
    let threshold = if open { 0.82 } else { 0.85 };
    let severity = if p >= 0.92 {
        Severity::Critical
    } else {
        Severity::Warn
    };
    (p >= threshold).then(|| Finding {
        subject,
        code: IncidentCode::DiskFilling,
        severity,
        detail: format!("{:.0}% {what}", p * 100.0),
    })
}

fn oom(input: &JudgeInput) -> Vec<Finding> {
    let host = &input.report.host;
    let counts = input
        .recent
        .oom_events
        .iter()
        .filter_map(|e| match e {
            LocalEvent::OomKilled {
                workload: Some(k), ..
            } => Some(Subject::Workload(k.clone())),
            LocalEvent::OomKilled { workload: None, .. } => Some(Subject::Host(host.clone())),
            LocalEvent::Unknown => None,
        })
        .fold(BTreeMap::<Subject, u32>::new(), |mut acc, s| {
            *acc.entry(s).or_default() += 1;
            acc
        });
    counts
        .into_iter()
        .map(|(subject, n)| Finding {
            subject,
            code: IncidentCode::OomKilled,
            severity: if n >= 3 {
                Severity::Critical
            } else {
                Severity::Warn
            },
            detail: format!("{n} OOM kills in the last hour"),
        })
        .collect()
}

/// Per workload, application class only. Components starting with `_` are skym's own:
/// `_stderr` is counted separately, other reserved components (protocol errors) never alarm.
fn app_exceptions(input: &JudgeInput) -> Vec<Finding> {
    let sums = input
        .recent
        .exceptions
        .iter()
        .filter(|g| g.class == ExceptionClass::Application)
        .fold(BTreeMap::<WorkloadKey, [u32; 3]>::new(), |mut acc, g| {
            let [f, r, s] = acc.entry(g.workload.clone()).or_default();
            if g.component == "_stderr" {
                *s += g.count;
            } else if !g.component.starts_with('_') {
                *f += g.final_count;
                *r += g.count.saturating_sub(g.final_count);
            }
            acc
        });
    sums.into_iter()
        .filter(|(_, [f, r, s])| *f >= 5 || *r >= 50 || *s >= 10)
        .map(|(key, [f, r, s])| Finding {
            subject: Subject::Workload(key),
            code: IncidentCode::AppExceptions,
            severity: if f >= 20 {
                Severity::Critical
            } else {
                Severity::Warn
            },
            detail: format!("final {f}, retried {r}, stderr {s} in 15m"),
        })
        .collect()
}
