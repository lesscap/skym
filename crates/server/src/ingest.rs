//! `POST /api/report`: one report, one transaction.

use crate::diff;
use crate::evaluate;
use crate::findings::{merge, projection};
use crate::lifecycle::State;
use crate::store::{history, hosts, incidents};
use jiff::{SignedDuration, Timestamp};
use rusqlite::Connection;
use skym_core::judge::{JudgeInput, Recent, judge};
use skym_core::model::WorkloadFacts;
use skym_core::report::{Report, rehost, validate};
use skym_core::rules::{Finding, IncidentCode};
use skym_core::subject::{Subject, WorkloadKey};
use skym_core::time::format_duration;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Accepted,
    /// Not newer than the last report: only proves the host is alive.
    Duplicate,
}

#[derive(Debug)]
pub enum IngestError {
    Invalid(String),
    Internal(anyhow::Error),
}

impl From<rusqlite::Error> for IngestError {
    fn from(e: rusqlite::Error) -> Self {
        IngestError::Internal(e.into())
    }
}

/// A clock this far ahead would make every later report look old.
pub const MAX_AHEAD: SignedDuration = SignedDuration::from_mins(10);
/// Agents buffer undelivered reports for a day; anything much older is not a real report.
const MAX_AGE: SignedDuration = SignedDuration::from_hours(24 * 7);

/// `host` comes from the token; the report's own host fields are replaced by it.
pub fn ingest(
    conn: &mut Connection,
    host: &str,
    body: &[u8],
    now: Timestamp,
) -> Result<Outcome, IngestError> {
    let report: Report = serde_json::from_slice(body)
        .map_err(|e| IngestError::Invalid(format!("not a report: {e}")))?;
    // Stored times have whole seconds; compare and judge at the same precision.
    let ts = Timestamp::from_second(report.ts.as_second())
        .map_err(|e| IngestError::Invalid(e.to_string()))?;
    let report = Report { ts, ..rehost(report, host) };
    validate(&report).map_err(|e| IngestError::Invalid(e.to_string()))?;
    if report.ts > now + MAX_AHEAD || report.ts < now - MAX_AGE {
        return Err(IngestError::Invalid(
            "report time is more than 10 minutes ahead or 7 days behind the server".into(),
        ));
    }
    let tx = conn.transaction()?;
    let prev = hosts::get(&tx, host)?;
    if prev.as_ref().is_some_and(|p| report.ts <= p.last_report_ts) {
        hosts::touch(&tx, host, now)?;
        tx.commit()?;
        return Ok(Outcome::Duplicate);
    }
    let stored = hosts::stored_facts(&tx, host)?;
    let events = diff::events(prev.as_ref().and_then(|p| p.facts.as_ref()), &stored, &report);
    history::insert_events(&tx, host, &events)?;
    hosts::upsert(&tx, &report, now)?;
    let agent = diff::reporting_agent(&report);
    for w in &report.workloads {
        hosts::upsert_workload(&tx, w, agent, now)?;
    }
    for m in &report.host_state.mounts {
        hosts::insert_disk_sample(&tx, host, report.ts, m)?;
    }
    history::insert_exceptions(&tx, host, report.ts, &report.exceptions)?;
    evaluate_host(&tx, &report)?;
    tx.commit()?;
    Ok(Outcome::Accepted)
}

/// Judges the report with the stored history, at the report's own time.
fn evaluate_host(c: &Connection, r: &Report) -> rusqlite::Result<()> {
    let (host, at) = (&r.host, r.ts);
    let facts: BTreeMap<WorkloadKey, WorkloadFacts> =
        hosts::workloads(c, host)?.into_iter().filter_map(|w| Some((w.key, w.facts?))).collect();
    let oom = history::oom_kills(c, host, at - SignedDuration::from_hours(1), at)?;
    let exceptions =
        history::exceptions(c, host, (at - SignedDuration::from_mins(15), at), None, None)?;
    let active = incidents::active_for_host(c, host)?;
    let open: BTreeSet<(Subject, IncidentCode)> = active
        .iter()
        .filter(|i| i.state == State::Open)
        .map(|i| (i.subject.clone(), i.code))
        .collect();
    let recent = Recent { oom_events: &oom, exceptions: &exceptions };
    let mut found = judge(&JudgeInput { report: r, facts: &facts, recent, open: &open });
    found.extend(projections(c, r, &open)?);
    let observed = |s: &Subject| r.errors.is_empty() || reported(r, s);
    evaluate::apply(c, &merge(found), &active, observed, at)
}

fn projections(
    c: &Connection,
    r: &Report,
    open: &BTreeSet<(Subject, IncidentCode)>,
) -> rusqlite::Result<Vec<Finding>> {
    let since = r.ts - SignedDuration::from_hours(6);
    let mut found = Vec::new();
    for m in &r.host_state.mounts {
        let subject = Subject::Mount { host: r.host.clone(), path: m.path.clone() };
        let samples = hosts::disk_samples(c, &r.host, &m.path, since)?;
        let is_open = open.contains(&(subject.clone(), IncidentCode::DiskFilling));
        if let Some((severity, eta)) = projection(&samples, m.total_bytes, is_open) {
            let detail = format!("full in ~{}", format_duration(eta));
            found.push(Finding { subject, code: IncidentCode::DiskFilling, severity, detail });
        }
    }
    Ok(found)
}

/// Whether the report says anything about the subject (when a source failed, absent
/// subjects are unknown rather than recovered).
fn reported(r: &Report, s: &Subject) -> bool {
    match s {
        Subject::Host(_) => true,
        Subject::Mount { path, .. } => r.host_state.mounts.iter().any(|m| &m.path == path),
        Subject::Workload(k) => r.workloads.iter().any(|w| &w.key == k),
        Subject::Endpoint(_) => false,
    }
}
