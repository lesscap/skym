//! Findings → incidents, for one scope. Ingest evaluates a host's own subjects; the
//! heartbeat task evaluates `HEARTBEAT_LOST`; the probe task evaluates endpoints. None of
//! them resolves another's incidents.

use crate::config::Endpoint;
use crate::findings;
use crate::lifecycle::{self, Incident};
use crate::probe::Probe;
use crate::store::{hosts, incidents, probes};
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

/// One probe pass over the configured endpoints; `probed` may miss some (not seen this pass).
/// Incidents of endpoints no longer configured are retired. A certificate is only judged when
/// the endpoint answered.
pub fn probes_once(
    conn: &mut Connection,
    configured: &[Endpoint],
    probed: &[(Endpoint, Probe)],
    now: Timestamp,
) -> anyhow::Result<()> {
    let tx = conn.transaction()?;
    let probed: BTreeMap<Subject, (&Endpoint, &Probe)> =
        probed.iter().map(|(e, p)| (Subject::Endpoint(e.url.clone()), (e, p))).collect();
    for (e, p) in probed.values() {
        probes::save(&tx, &e.url, p)?;
    }
    let urls: Vec<&str> = configured.iter().map(|e| e.url.as_str()).collect();
    probes::prune(&tx, &urls)?;
    let (active, gone): (Vec<Incident>, Vec<Incident>) = incidents::active_endpoints(&tx)?
        .into_iter()
        .partition(|i| matches!(&i.subject, Subject::Endpoint(u) if urls.contains(&u.as_str())));
    for i in &gone {
        incidents::apply(&tx, &lifecycle::retire(i, now), now)?;
    }
    let found: Vec<Finding> =
        probed.values().flat_map(|(e, p)| findings::endpoint(&e.url, &e.expect, p)).collect();
    let observed = |s: &Subject, code| {
        probed.get(s).is_some_and(|(_, p)| code != IncidentCode::CertExpiring || p.response.is_ok())
    };
    apply(&tx, &found, &active, observed, now)?;
    tx.commit()?;
    Ok(())
}
