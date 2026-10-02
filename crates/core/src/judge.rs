//! Turns one report into findings. Pure: the caller supplies recent history and
//! which incidents are already open (to pick resolve thresholds).

use crate::model::{
    ExceptionClass, ExceptionGroup, Health, LocalEvent, MountState, RunState, WorkloadFacts,
    WorkloadState,
};
use crate::report::{Report, WorkloadReport};
use crate::rules::{Finding, IncidentCode, Severity};
use crate::subject::{Subject, WorkloadKey};
use crate::time::SignedDuration;
use std::collections::{BTreeMap, BTreeSet};

/// History the report alone does not carry, already filtered by the caller.
pub struct Recent<'a> {
    /// OOM kills within the last hour.
    pub oom_events: &'a [LocalEvent],
    /// Exception groups within the last 15 minutes.
    pub exceptions: &'a [ExceptionGroup],
}

pub struct JudgeInput<'a> {
    /// Rehosted and validated (see [`crate::report::validate`]).
    /// Its `ts` is the reference time for every window.
    pub report: &'a Report,
    /// Known facts for workloads whose report omits them.
    pub facts: &'a BTreeMap<WorkloadKey, WorkloadFacts>,
    pub recent: Recent<'a>,
    pub open: &'a BTreeSet<(Subject, IncidentCode)>,
}

impl JudgeInput<'_> {
    fn is_open(&self, subject: &Subject, code: IncidentCode) -> bool {
        self.open.contains(&(subject.clone(), code))
    }
}

/// At most one finding per `(subject, code)`.
pub fn judge(input: &JudgeInput) -> Vec<Finding> {
    let report = input.report;
    let workloads = report.workloads.iter().flat_map(|w| workload(input, w));
    let mounts = report.host_state.mounts.iter().filter_map(|m| disk(input, m));
    workloads
        .chain(mounts)
        .chain(oom(input))
        .chain(log_unbounded(input))
        .chain(app_exceptions(input))
        .collect()
}

fn workload(input: &JudgeInput, w: &WorkloadReport) -> Vec<Finding> {
    let subject = &Subject::Workload(w.key.clone());
    let state = &w.state;
    [
        down(subject, state),
        unhealthy(subject, state),
        crash_loop(subject, state, input),
        datastore_unreachable(subject, state),
        replication_lag(subject, state, input),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn finding(subject: &Subject, code: IncidentCode, severity: Severity, detail: String) -> Finding {
    Finding { subject: subject.clone(), code, severity, detail }
}

fn critical_if(critical: bool) -> Severity {
    if critical { Severity::Critical } else { Severity::Warn }
}

fn down(subject: &Subject, state: &WorkloadState) -> Option<Finding> {
    let not_running = match (state.run, state.exit_code) {
        (RunState::Running | RunState::Unknown, _) | (RunState::Exited, Some(0)) => None,
        (RunState::Exited, Some(code)) => Some(format!("exited ({code})")),
        (run, _) => Some(run.to_string()),
    };
    let not_listening = || {
        let ports: Vec<String> = state.missing_ports.iter().map(u16::to_string).collect();
        (!ports.is_empty()).then(|| format!("not listening on {}", ports.join(", ")))
    };
    let detail = not_running.or_else(not_listening)?;
    Some(finding(subject, IncidentCode::WorkloadDown, Severity::Critical, detail))
}

fn unhealthy(subject: &Subject, state: &WorkloadState) -> Option<Finding> {
    let detail = "healthcheck unhealthy".to_string();
    (state.health == Some(Health::Unhealthy))
        .then(|| finding(subject, IncidentCode::WorkloadUnhealthy, Severity::Critical, detail))
}

fn crash_loop(subject: &Subject, state: &WorkloadState, input: &JudgeInput) -> Option<Finding> {
    let now = input.report.ts;
    let within = |window| state.restarts.iter().filter(|t| **t > now - window).count();
    let last_hour = within(SignedDuration::from_hours(1));
    let matched = if input.is_open(subject, IncidentCode::CrashLoop) {
        within(SignedDuration::from_mins(30)) >= 1
    } else {
        last_hour >= 3
    };
    let detail = format!("{last_hour} restarts in the last hour");
    matched.then(|| finding(subject, IncidentCode::CrashLoop, critical_if(last_hour >= 10), detail))
}

/// The `local` driver rotates by default; only `json-file` grows without a limit.
/// One finding per host: unbounded logs are fixed host by host, and one warning per
/// container would bury everything else.
fn log_unbounded(input: &JudgeInput) -> Option<Finding> {
    let unbounded: Vec<String> = input
        .report
        .workloads
        .iter()
        .filter(|w| {
            let facts = w.facts.as_ref().or_else(|| input.facts.get(&w.key));
            facts.is_some_and(|f| {
                f.log_driver.as_deref() == Some("json-file") && f.log_max_size.is_none()
            })
        })
        .map(|w| match w.key.project.as_str() {
            "-" => w.key.service.clone(),
            project => format!("{project}/{}", w.key.service),
        })
        .collect();
    if unbounded.is_empty() {
        return None;
    }
    let more = unbounded.len().saturating_sub(5);
    let names = unbounded.iter().take(5).cloned().collect::<Vec<_>>().join(", ");
    let detail = format!(
        "{} to json-file without max-size: {names}{}",
        match unbounded.len() {
            1 => "1 container logs".to_string(),
            n => format!("{n} containers log"),
        },
        if more > 0 { format!(" and {more} more") } else { String::new() }
    );
    let host = Subject::Host(input.report.host.clone());
    Some(finding(&host, IncidentCode::LogUnbounded, Severity::Warn, detail))
}

fn datastore_unreachable(subject: &Subject, state: &WorkloadState) -> Option<Finding> {
    let probe = state.datastore.as_ref().filter(|d| !d.reachable)?;
    let code = IncidentCode::DatastoreUnreachable;
    Some(finding(subject, code, Severity::Critical, probe.detail.clone()))
}

fn replication_lag(
    subject: &Subject,
    state: &WorkloadState,
    input: &JudgeInput,
) -> Option<Finding> {
    let lag = state.datastore.as_ref()?.replication_lag_s?;
    let matched =
        if input.is_open(subject, IncidentCode::ReplicationLag) { lag >= 10.0 } else { lag > 30.0 };
    let detail = format!("replication lag {lag:.0}s");
    matched
        .then(|| finding(subject, IncidentCode::ReplicationLag, critical_if(lag > 300.0), detail))
}

fn disk(input: &JudgeInput, m: &MountState) -> Option<Finding> {
    let ratio = |used: u64, total: u64| (total > 0).then(|| used as f64 / total as f64);
    let (p, what) = [
        (ratio(m.used_bytes, m.total_bytes), "used"),
        (ratio(m.inodes_used, m.inodes_total), "inodes used"),
    ]
    .into_iter()
    .filter_map(|(r, what)| Some((r?, what)))
    .max_by(|a, b| a.0.total_cmp(&b.0))?;
    let subject = &Subject::Mount { host: input.report.host.clone(), path: m.path.clone() };
    let threshold = if input.is_open(subject, IncidentCode::DiskFilling) { 0.82 } else { 0.85 };
    let detail = format!("{:.0}% {what}", p * 100.0);
    (p >= threshold)
        .then(|| finding(subject, IncidentCode::DiskFilling, critical_if(p >= 0.92), detail))
}

fn oom(input: &JudgeInput) -> Vec<Finding> {
    let host = &input.report.host;
    let subject_of = |e: &LocalEvent| match e {
        LocalEvent::OomKilled { workload: Some(k), .. } => Some(Subject::Workload(k.clone())),
        LocalEvent::OomKilled { workload: None, .. } => Some(Subject::Host(host.clone())),
        LocalEvent::Unknown => None,
    };
    let counts = input.recent.oom_events.iter().filter_map(subject_of).fold(
        BTreeMap::<Subject, u32>::new(),
        |mut acc, s| {
            *acc.entry(s).or_default() += 1;
            acc
        },
    );
    counts
        .into_iter()
        .map(|(subject, n)| {
            let detail = format!("{n} OOM kills in the last hour");
            finding(&subject, IncidentCode::OomKilled, critical_if(n >= 3), detail)
        })
        .collect()
}

#[derive(Default)]
struct ExceptionCounts {
    gave_up: u32,
    retrying: u32,
    stderr: u32,
}

impl ExceptionCounts {
    /// `_stderr` is the unstructured fallback. Protocol errors point at the application's
    /// logging, not at a production failure, so they never alarm; `_OVERFLOW` does.
    fn add(&mut self, g: &ExceptionGroup) {
        if g.code == "_PROTOCOL_ERROR" {
            return;
        }
        if g.component == "_stderr" {
            self.stderr = self.stderr.saturating_add(g.count);
        } else {
            self.gave_up = self.gave_up.saturating_add(g.final_count);
            self.retrying = self.retrying.saturating_add(g.count.saturating_sub(g.final_count));
        }
    }

    fn alarming(&self) -> bool {
        self.gave_up >= 5 || self.retrying >= 50 || self.stderr >= 10
    }
}

/// Per workload, application class only.
fn app_exceptions(input: &JudgeInput) -> Vec<Finding> {
    let application =
        input.recent.exceptions.iter().filter(|g| g.class == ExceptionClass::Application);
    let per_workload =
        application.fold(BTreeMap::<&WorkloadKey, ExceptionCounts>::new(), |mut acc, g| {
            acc.entry(&g.workload).or_default().add(g);
            acc
        });
    per_workload
        .into_iter()
        .filter(|(_, c)| c.alarming())
        .map(|(key, c)| {
            let detail = format!(
                "gave up {}, retrying {}, stderr {} in 15m",
                c.gave_up, c.retrying, c.stderr
            );
            let severity = critical_if(c.gave_up >= 20);
            finding(&Subject::Workload(key.clone()), IncidentCode::AppExceptions, severity, detail)
        })
        .collect()
}
