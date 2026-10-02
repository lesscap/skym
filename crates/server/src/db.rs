//! SQLite: opening, migrations, and the one timestamp format stored everywhere.

use anyhow::Context;
use jiff::Timestamp;
use rusqlite::Connection;
use std::path::Path;

/// Applied in order; `PRAGMA user_version` records how many have run.
const MIGRATIONS: &[&str] = &[r#"
CREATE TABLE hosts (
  id             TEXT PRIMARY KEY,
  facts_json     TEXT,
  facts_hash     TEXT,
  state_json     TEXT NOT NULL,
  errors_json    TEXT NOT NULL,
  last_report_ts TEXT NOT NULL,
  last_seen      TEXT NOT NULL
);
CREATE TABLE workloads (
  host        TEXT NOT NULL,
  project     TEXT NOT NULL,
  service     TEXT NOT NULL,
  facts_json  TEXT,
  facts_hash  TEXT,
  agent_version TEXT,
  state_json  TEXT NOT NULL,
  last_seen   TEXT NOT NULL,
  archived_at TEXT,
  PRIMARY KEY (host, project, service)
) WITHOUT ROWID;
CREATE TABLE disk_samples (
  host        TEXT NOT NULL,
  path        TEXT NOT NULL,
  ts          TEXT NOT NULL,
  used_bytes  INTEGER NOT NULL,
  total_bytes INTEGER NOT NULL,
  PRIMARY KEY (host, path, ts)
) WITHOUT ROWID;
CREATE TABLE events (
  id        INTEGER PRIMARY KEY,
  host      TEXT NOT NULL,
  subject   TEXT NOT NULL,
  ts        TEXT NOT NULL,
  kind      TEXT NOT NULL,
  kind_json TEXT NOT NULL,
  UNIQUE (subject, ts, kind)
);
CREATE INDEX events_host_ts ON events (host, ts);
CREATE TABLE incidents (
  id             INTEGER PRIMARY KEY,
  host           TEXT,
  subject        TEXT NOT NULL,
  code           TEXT NOT NULL,
  state          TEXT NOT NULL,
  severity       TEXT NOT NULL,
  peak_severity  TEXT NOT NULL,
  detail         TEXT NOT NULL,
  match_streak   INTEGER NOT NULL,
  clear_streak   INTEGER NOT NULL,
  first_match_at TEXT NOT NULL,
  opened_at      TEXT,
  last_seen      TEXT NOT NULL,
  resolved_at    TEXT,
  CHECK ((state = 'pending'  AND opened_at IS NULL     AND resolved_at IS NULL)
      OR (state = 'open'     AND opened_at IS NOT NULL AND resolved_at IS NULL)
      OR (state = 'resolved' AND opened_at IS NOT NULL AND resolved_at IS NOT NULL))
);
CREATE UNIQUE INDEX incidents_active ON incidents (subject, code) WHERE state <> 'resolved';
CREATE INDEX incidents_host ON incidents (host, state);
CREATE INDEX incidents_resolved ON incidents (subject, code, resolved_at) WHERE state = 'resolved';
CREATE TABLE incident_log (
  incident_id INTEGER NOT NULL REFERENCES incidents (id) ON DELETE CASCADE,
  ts          TEXT NOT NULL,
  change      TEXT NOT NULL CHECK (change IN ('opened', 'reopened', 'resolved', 'severity')),
  severity    TEXT NOT NULL,
  detail      TEXT NOT NULL
);
CREATE INDEX incident_log_ts ON incident_log (ts);
CREATE INDEX incident_log_incident ON incident_log (incident_id);
CREATE TABLE exception_groups (
  host          TEXT NOT NULL,
  project       TEXT NOT NULL,
  service       TEXT NOT NULL,
  class         TEXT NOT NULL,
  component     TEXT NOT NULL,
  code          TEXT NOT NULL,
  report_ts     TEXT NOT NULL,
  count         INTEGER NOT NULL,
  final_count   INTEGER NOT NULL,
  first_seen    TEXT NOT NULL,
  last_seen     TEXT NOT NULL,
  biz_keys_json TEXT NOT NULL,
  sample_json   TEXT
);
CREATE INDEX exception_groups_host ON exception_groups (host, report_ts);
"#];

pub fn open(path: &Path) -> anyhow::Result<Connection> {
    let mut conn = Connection::open(path).with_context(|| format!("open {}", path.display()))?;
    prepare(&mut conn)?;
    Ok(conn)
}

pub fn open_in_memory() -> anyhow::Result<Connection> {
    let mut conn = Connection::open_in_memory()?;
    prepare(&mut conn)?;
    Ok(conn)
}

fn prepare(conn: &mut Connection) -> anyhow::Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    let applied: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(applied as usize) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql).with_context(|| format!("migration {}", i + 1))?;
        tx.pragma_update(None, "user_version", i as i64 + 1)?;
        tx.commit()?;
    }
    Ok(())
}

/// Whole seconds, `…Z`: text order equals time order.
pub fn ts(t: Timestamp) -> String {
    Timestamp::from_second(t.as_second()).expect("in range").to_string()
}

pub fn parse_ts(s: &str) -> rusqlite::Result<Timestamp> {
    s.parse().map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_timestamps_sort_as_time() {
        let a: Timestamp = "2026-10-01T12:00:00.900Z".parse().unwrap();
        let b: Timestamp = "2026-10-01T12:00:01Z".parse().unwrap();
        assert_eq!(ts(a), "2026-10-01T12:00:00Z");
        assert!(ts(a) < ts(b));
        assert_eq!(parse_ts(&ts(b)).unwrap(), b);
    }

    #[test]
    fn migrations_apply_once() {
        let conn = open_in_memory().unwrap();
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
        assert_eq!(version as usize, MIGRATIONS.len());
    }
}
