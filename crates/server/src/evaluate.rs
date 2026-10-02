//! Findings → incidents, for one scope. Ingest evaluates a host's own subjects; the
//! heartbeat task evaluates `HEARTBEAT_LOST`; the two never resolve each other's incidents.

use crate::findings;
use crate::lifecycle::{self, Incident};
use crate::store::{hosts, incidents};
use jiff::{SignedDuration, Timestamp};
use rusqlite::Connection;
use skym_core::rules::{Finding, IncidentCode, rule};
use skym_core::subject::{HostId, Subject};
use std::collections::{BTreeMap, BTreeSet};

/// Runs the lifecycle for each `(subject, code)` with a finding or an active incident.
/// Subjects the pass did not observe keep their incidents (their new findings still count).
pub fn apply(
    c: &Connection,
    found: &[Finding],
    active: &[Incident],
    observed: impl Fn(&Subject, IncidentCode) -> bool,
    now: Timestamp,
) -> rusqlite::Result<()> {
    let keys: BTreeSet<(&Subject, IncidentCode)> = found
        .iter()
        .map(|f| (&f.subject, f.code))
        .chain(active.iter().map(|i| (&i.subject, i.code)))
        .collect();
    for (subject, code) in keys {
        let finding = found.iter().find(|f| f.subject == *subject && f.code == code);
        if finding.is_none() && !observed(subject, code) {
            continue; // not seen this time: unknown, not recovered
        }
        let current = active.iter().find(|i| i.subject == *subject && i.code == code);
        let reopenable = incidents::latest_resolved(c, subject, code)?;
        let host = subject.host().cloned();
        let t =
            lifecycle::next(rule(code), current, reopenable.as_ref(), finding, host.as_ref(), now);
        incidents::apply(c, &t, now)?;
    }
    Ok(())
}

/// One heartbeat evaluation over the configured hosts.
pub fn heartbeat_once(
    conn: &mut Connection,
    configured: &[HostId],
    started: Timestamp,
    interval: SignedDuration,
    now: Timestamp,
) -> anyhow::Result<()> {
    let tx = conn.transaction()?;
    let seen: BTreeMap<HostId, Timestamp> =
        hosts::all(&tx)?.into_iter().map(|h| (h.id, h.last_seen)).collect();
    let list: Vec<(HostId, Option<Timestamp>)> =
        configured.iter().map(|h| (h.clone(), seen.get(h).copied())).collect();
    let found = findings::heartbeat(&list, started, interval, now);
    let active = incidents::active_with_code(&tx, IncidentCode::HeartbeatLost)?;
    let observed = |s: &Subject, _| {
        let seen = s.host().and_then(|h| seen.get(h).copied());
        !findings::in_grace(seen, started, interval, now)
    };
    apply(&tx, &found, &active, observed, now)?;
    tx.commit()?;
    Ok(())
}
