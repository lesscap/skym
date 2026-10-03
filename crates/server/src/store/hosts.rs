//! Latest facts and state per host and workload, and disk usage samples.

use super::{from_json, json, parsed, ts};
use crate::diff::Stored;
use crate::lifecycle::Incident;
use jiff::Timestamp;
use rusqlite::{Connection, OptionalExtension, params};
use skym_core::model::{HostFacts, HostState, MountState, WorkloadFacts, WorkloadState};
use skym_core::report::{Report, WorkloadReport};
use skym_core::rules::IncidentCode;
use skym_core::subject::{HostId, Subject, WorkloadKey};
use std::collections::BTreeMap;

pub struct HostRow {
    pub id: HostId,
    pub facts: Option<HostFacts>,
    pub state: HostState,
    pub errors: Vec<String>,
    pub last_report_ts: Timestamp,
    pub last_seen: Timestamp,
    /// When skym first heard from the host.
    pub first_seen: Timestamp,
}

pub struct WorkloadRow {
    pub key: WorkloadKey,
    pub facts: Option<WorkloadFacts>,
    pub state: WorkloadState,
    pub last_seen: Timestamp,
}

const HOST_COLUMNS: &str = "id, facts_json, state_json, errors_json, last_report_ts, last_seen, \
     COALESCE(first_seen, last_seen)";

fn host_row(r: &rusqlite::Row) -> rusqlite::Result<HostRow> {
    Ok(HostRow {
        id: r.get(0)?,
        facts: r.get::<_, Option<String>>(1)?.map(|f| from_json(&f)).transpose()?,
        state: from_json(&r.get::<_, String>(2)?)?,
        errors: from_json(&r.get::<_, String>(3)?)?,
        last_report_ts: parsed(r.get(4)?)?,
        last_seen: parsed(r.get(5)?)?,
        first_seen: parsed(r.get(6)?)?,
    })
}

pub fn get(c: &Connection, id: &str) -> rusqlite::Result<Option<HostRow>> {
    c.query_row(&format!("SELECT {HOST_COLUMNS} FROM hosts WHERE id = ?1"), [id], host_row)
        .optional()
}

pub fn all(c: &Connection) -> rusqlite::Result<Vec<HostRow>> {
    c.prepare(&format!("SELECT {HOST_COLUMNS} FROM hosts"))?.query_map([], host_row)?.collect()
}

/// A report from the host arrived (even a duplicate): it is alive.
pub fn touch(c: &Connection, id: &str, now: Timestamp) -> rusqlite::Result<()> {
    c.execute("UPDATE hosts SET last_seen = ?2 WHERE id = ?1", params![id, ts(now)]).map(drop)
}

/// Stores the report's host part. Facts are kept when the report omits them.
pub fn upsert(c: &Connection, r: &Report, now: Timestamp) -> rusqlite::Result<()> {
    c.execute(
        "INSERT INTO hosts (id, facts_json, facts_hash, state_json, errors_json, last_report_ts, last_seen, first_seen)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
         ON CONFLICT (id) DO UPDATE SET
           facts_json = COALESCE(excluded.facts_json, facts_json),
           facts_hash = COALESCE(excluded.facts_hash, facts_hash),
           state_json = excluded.state_json, errors_json = excluded.errors_json,
           last_report_ts = excluded.last_report_ts, last_seen = excluded.last_seen",
        params![
            r.host,
            r.host_facts.as_ref().map(json),
            r.host_facts.as_ref().map(|_| &r.host_facts_hash),
            json(&r.host_state),
            json(&r.errors),
            ts(r.ts),
            ts(now)
        ],
    )
    .map(drop)
}

/// `agent` is the version of the agent that sent the report; it is stored with new facts.
pub fn upsert_workload(
    c: &Connection,
    w: &WorkloadReport,
    agent: Option<&str>,
    now: Timestamp,
) -> rusqlite::Result<()> {
    c.execute(
        "INSERT INTO workloads (host, project, service, facts_json, facts_hash, agent_version, state_json, last_seen, archived_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL)
         ON CONFLICT (host, project, service) DO UPDATE SET
           facts_json = COALESCE(excluded.facts_json, facts_json),
           facts_hash = COALESCE(excluded.facts_hash, facts_hash),
           agent_version = CASE WHEN excluded.facts_json IS NULL THEN agent_version ELSE excluded.agent_version END,
           state_json = excluded.state_json, last_seen = excluded.last_seen, archived_at = NULL",
        params![
            w.key.host,
            w.key.project,
            w.key.service,
            w.facts.as_ref().map(json),
            w.facts.as_ref().map(|_| &w.facts_hash),
            agent,
            json(&w.state),
            ts(now)
        ],
    )
    .map(drop)
}

/// Stored facts of a host's workloads, raw, for diffing.
pub fn stored_facts(c: &Connection, host: &str) -> rusqlite::Result<BTreeMap<WorkloadKey, Stored>> {
    let mut stmt = c.prepare(
        "SELECT project, service, facts_json, agent_version FROM workloads WHERE host = ?1 AND facts_json IS NOT NULL",
    )?;
    let rows = stmt.query_map([host], |r| {
        let key = WorkloadKey { host: host.to_string(), project: r.get(0)?, service: r.get(1)? };
        Ok((key, Stored { facts: from_json(&r.get::<_, String>(2)?)?, agent: r.get(3)? }))
    })?;
    rows.collect()
}

/// The host's workloads that are not archived.
pub fn workloads(c: &Connection, host: &str) -> rusqlite::Result<Vec<WorkloadRow>> {
    workload_rows(c, Some(host))
}

/// Every host's workloads that are not archived.
pub fn all_workloads(c: &Connection) -> rusqlite::Result<Vec<WorkloadRow>> {
    workload_rows(c, None)
}

fn workload_rows(c: &Connection, host: Option<&str>) -> rusqlite::Result<Vec<WorkloadRow>> {
    let mut stmt = c.prepare(
        "SELECT host, project, service, facts_json, state_json, last_seen FROM workloads
         WHERE (?1 IS NULL OR host = ?1) AND archived_at IS NULL ORDER BY host, project, service",
    )?;
    let rows = stmt.query_map([host], |r| {
        Ok(WorkloadRow {
            key: WorkloadKey { host: r.get(0)?, project: r.get(1)?, service: r.get(2)? },
            facts: r.get::<_, Option<String>>(3)?.map(|f| from_json(&f)).transpose()?,
            state: from_json(&r.get::<_, String>(4)?)?,
            last_seen: parsed(r.get(5)?)?,
        })
    })?;
    rows.collect()
}

pub fn insert_disk_sample(
    c: &Connection,
    host: &str,
    at: Timestamp,
    m: &MountState,
) -> rusqlite::Result<()> {
    c.execute(
        "INSERT OR IGNORE INTO disk_samples (host, path, ts, used_bytes, total_bytes) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![host, m.path, ts(at), m.used_bytes as i64, m.total_bytes as i64],
    )
    .map(drop)
}

pub fn disk_samples(
    c: &Connection,
    host: &str,
    path: &str,
    since: Timestamp,
) -> rusqlite::Result<Vec<(Timestamp, u64)>> {
    let mut stmt = c.prepare("SELECT ts, used_bytes FROM disk_samples WHERE host = ?1 AND path = ?2 AND ts >= ?3 ORDER BY ts")?;
    let rows = stmt.query_map(params![host, path, ts(since)], |r| {
        Ok((parsed(r.get(0)?)?, r.get::<_, i64>(1)? as u64))
    })?;
    rows.collect()
}

/// Workloads not reported for a week are archived (deleted containers, old replicas).
pub fn archive_stale(c: &Connection, before: Timestamp, now: Timestamp) -> rusqlite::Result<usize> {
    c.execute(
        "UPDATE workloads SET archived_at = ?2 WHERE archived_at IS NULL AND last_seen < ?1",
        params![ts(before), ts(now)],
    )
}

pub fn archived(c: &Connection) -> rusqlite::Result<Vec<WorkloadKey>> {
    let mut stmt =
        c.prepare("SELECT host, project, service FROM workloads WHERE archived_at IS NOT NULL")?;
    let rows = stmt.query_map([], |r| {
        Ok(WorkloadKey { host: r.get(0)?, project: r.get(1)?, service: r.get(2)? })
    })?;
    rows.collect()
}

pub fn prune_archived(c: &Connection, before: Timestamp) -> rusqlite::Result<usize> {
    c.execute("DELETE FROM workloads WHERE archived_at < ?1", [ts(before)])
}

pub fn prune_disk_samples(c: &Connection, before: Timestamp) -> rusqlite::Result<usize> {
    c.execute("DELETE FROM disk_samples WHERE ts < ?1", [ts(before)])
}

/// When each stopped workload among these incidents stopped, if its state says so.
pub fn stopped_since(
    c: &Connection,
    open: &[Incident],
) -> rusqlite::Result<BTreeMap<WorkloadKey, Timestamp>> {
    let mut stmt = c.prepare(
        "SELECT state_json FROM workloads WHERE host = ?1 AND project = ?2 AND service = ?3",
    )?;
    let mut stopped = BTreeMap::new();
    for i in open {
        if let (IncidentCode::WorkloadDown, Subject::Workload(k)) = (i.code, &i.subject) {
            let state: Option<String> =
                stmt.query_row(params![k.host, k.project, k.service], |r| r.get(0)).optional()?;
            let state = state.map(|s| from_json::<WorkloadState>(&s)).transpose()?;
            if let Some(t) = state.as_ref().and_then(WorkloadState::stopped_since) {
                stopped.insert(k.clone(), t);
            }
        }
    }
    Ok(stopped)
}
