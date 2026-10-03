//! Incident rows and their change log.

use super::{parsed, ts};
use crate::lifecycle::{Change, Incident, Transition};
use jiff::Timestamp;
use rusqlite::{Connection, params};
use skym_core::rules::{IncidentCode, Severity};
use skym_core::subject::Subject;

/// Every column but `id`, in `apply`'s parameter order.
const FIELDS: &str = "host, subject, code, state, severity, peak_severity, detail, match_streak, \
                      clear_streak, first_match_at, opened_at, last_seen, resolved_at";

const PLACEHOLDERS: &str = "?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13";

fn row(r: &rusqlite::Row) -> rusqlite::Result<Incident> {
    Ok(Incident {
        id: Some(r.get(0)?),
        host: r.get(1)?,
        subject: parsed(r.get(2)?)?,
        code: parsed(r.get(3)?)?,
        state: parsed(r.get(4)?)?,
        severity: parsed(r.get(5)?)?,
        peak_severity: parsed(r.get(6)?)?,
        detail: r.get(7)?,
        match_streak: r.get(8)?,
        clear_streak: r.get(9)?,
        first_match_at: parsed(r.get(10)?)?,
        opened_at: r.get::<_, Option<String>>(11)?.map(parsed).transpose()?,
        last_seen: parsed(r.get(12)?)?,
        resolved_at: r.get::<_, Option<String>>(13)?.map(parsed).transpose()?,
    })
}

fn query(
    c: &Connection,
    filter: &str,
    args: impl rusqlite::Params,
) -> rusqlite::Result<Vec<Incident>> {
    c.prepare(&format!("SELECT id, {FIELDS} FROM incidents WHERE {filter}"))?
        .query_map(args, row)?
        .collect()
}

/// Pending and open incidents of a host's own subjects (heartbeats have their own scope).
pub fn active_for_host(c: &Connection, host: &str) -> rusqlite::Result<Vec<Incident>> {
    query(c, "host = ?1 AND state <> 'resolved' AND code <> 'HEARTBEAT_LOST'", [host])
}

pub fn active_with_code(c: &Connection, code: IncidentCode) -> rusqlite::Result<Vec<Incident>> {
    query(c, "code = ?1 AND state <> 'resolved'", [code.as_str()])
}

/// Pending and open incidents of endpoints, which belong to no host.
pub fn active_endpoints(c: &Connection) -> rusqlite::Result<Vec<Incident>> {
    query(c, "host IS NULL AND subject LIKE 'endpoint:%' AND state <> 'resolved'", [])
}

pub fn latest_resolved(
    c: &Connection,
    subject: &Subject,
    code: IncidentCode,
) -> rusqlite::Result<Option<Incident>> {
    let filter =
        "subject = ?1 AND code = ?2 AND state = 'resolved' ORDER BY resolved_at DESC LIMIT 1";
    Ok(query(c, filter, params![subject.to_string(), code.as_str()])?.pop())
}

/// Open incidents (whatever their age), or those resolved since `since`; newest first.
pub fn listed(
    c: &Connection,
    open: bool,
    host: Option<&str>,
    code: Option<IncidentCode>,
    since: Timestamp,
    limit: usize,
) -> rusqlite::Result<Vec<Incident>> {
    let state = if open { "state = 'open'" } else { "state = 'resolved' AND resolved_at >= ?2" };
    let filter = format!(
        "{state} AND (?1 IS NULL OR host = ?1) AND (?4 IS NULL OR code = ?4) ORDER BY last_seen DESC LIMIT ?3"
    );
    query(c, &filter, params![host, ts(since), limit as i64, code.map(IncidentCode::as_str)])
}

/// Pending and open incidents of one subject.
pub fn active_for(c: &Connection, subject: &Subject) -> rusqlite::Result<Vec<Incident>> {
    query(c, "subject = ?1 AND state <> 'resolved'", [subject.to_string()])
}

/// Deletes a replaced pending row first: at most one active row per (subject, code).
pub fn apply(c: &Connection, t: &Transition, now: Timestamp) -> rusqlite::Result<()> {
    if let Some(id) = t.delete {
        c.execute("DELETE FROM incidents WHERE id = ?1", [id])?;
    }
    let Some((incident, change)) = &t.write else { return Ok(()) };
    let values = params![
        incident.host,
        incident.subject.to_string(),
        incident.code.as_str(),
        incident.state.as_str(),
        incident.severity.as_str(),
        incident.peak_severity.as_str(),
        incident.detail,
        incident.match_streak,
        incident.clear_streak,
        ts(incident.first_match_at),
        incident.opened_at.map(ts),
        ts(incident.last_seen),
        incident.resolved_at.map(ts),
    ];
    let id = match incident.id {
        Some(id) => {
            let sql = format!("UPDATE incidents SET ({FIELDS}) = ({PLACEHOLDERS}) WHERE id = ?14");
            let mut args: Vec<&dyn rusqlite::ToSql> = values.to_vec();
            args.push(&id);
            c.execute(&sql, args.as_slice())?;
            id
        }
        None => {
            let sql = format!("INSERT INTO incidents ({FIELDS}) VALUES ({PLACEHOLDERS})");
            c.execute(&sql, values)?;
            c.last_insert_rowid()
        }
    };
    if let Some(change) = change {
        log(c, id, *change, incident, now)?;
    }
    Ok(())
}

fn log(
    c: &Connection,
    id: i64,
    change: Change,
    i: &Incident,
    now: Timestamp,
) -> rusqlite::Result<()> {
    c.execute(
        "INSERT INTO incident_log (incident_id, ts, change, severity, detail) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, ts(now), change.as_str(), i.severity.as_str(), i.detail],
    )
    .map(drop)
}

pub struct LogEntry {
    pub ts: Timestamp,
    pub subject: Subject,
    pub code: IncidentCode,
    pub change: Change,
    pub severity: Severity,
    pub detail: String,
}

/// Incident changes for a host (and optionally one subject), newest first.
pub fn changes(
    c: &Connection,
    host: &str,
    subject: Option<&Subject>,
    since: Timestamp,
    limit: usize,
) -> rusqlite::Result<Vec<LogEntry>> {
    let mut stmt = c.prepare(
        "SELECT l.ts, i.subject, i.code, l.change, l.severity, l.detail FROM incident_log l
         JOIN incidents i ON i.id = l.incident_id
         WHERE i.host = ?1 AND (?2 IS NULL OR i.subject = ?2) AND l.ts >= ?3
         ORDER BY l.ts DESC LIMIT ?4",
    )?;
    let rows = stmt.query_map(
        params![host, subject.map(Subject::to_string), ts(since), limit as i64],
        |r| {
            Ok(LogEntry {
                ts: parsed(r.get(0)?)?,
                subject: parsed(r.get(1)?)?,
                code: parsed(r.get(2)?)?,
                change: parsed(r.get(3)?)?,
                severity: parsed(r.get(4)?)?,
                detail: r.get(5)?,
            })
        },
    )?;
    rows.collect()
}

pub fn prune_resolved(c: &Connection, before: Timestamp) -> rusqlite::Result<usize> {
    c.execute("DELETE FROM incidents WHERE state = 'resolved' AND resolved_at < ?1", [ts(before)])
}
