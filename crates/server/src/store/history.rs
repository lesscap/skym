//! Events and exception groups, appended and queried by time.

use super::{from_json, json, parsed, ts};
use jiff::Timestamp;
use rusqlite::{Connection, params};
use skym_core::model::{Event, ExceptionClass, ExceptionGroup, LocalEvent};
use skym_core::subject::{Subject, WorkloadKey};
use std::collections::BTreeMap;

/// Idempotent: the same event (subject, time, kind) is stored once.
pub fn insert_events(c: &Connection, host: &str, events: &[Event]) -> rusqlite::Result<()> {
    let mut stmt = c.prepare(
        "INSERT OR IGNORE INTO events (host, subject, ts, kind, kind_json) VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;
    for e in events {
        stmt.execute(params![host, e.subject.to_string(), ts(e.ts), e.kind.tag(), json(&e.kind)])?;
    }
    Ok(())
}

/// A host's events (optionally of one subject) since `since`, newest first.
pub fn events(
    c: &Connection,
    host: &str,
    subject: Option<&Subject>,
    since: Timestamp,
    limit: usize,
) -> rusqlite::Result<Vec<Event>> {
    let mut stmt = c.prepare(
        "SELECT ts, subject, kind_json FROM events WHERE host = ?1 AND (?2 IS NULL OR subject = ?2)
         AND ts >= ?3 ORDER BY ts DESC LIMIT ?4",
    )?;
    let args = params![host, subject.map(Subject::to_string), ts(since), limit as i64];
    let rows = stmt.query_map(args, |r| {
        Ok(Event {
            ts: parsed(r.get(0)?)?,
            subject: parsed(r.get(1)?)?,
            kind: from_json(&r.get::<_, String>(2)?)?,
        })
    })?;
    rows.collect()
}

/// When each workload was last deployed, as far as the stored events go back.
pub fn last_deployed(c: &Connection) -> rusqlite::Result<BTreeMap<WorkloadKey, Timestamp>> {
    let mut stmt =
        c.prepare("SELECT subject, MAX(ts) FROM events WHERE kind = 'deployed' GROUP BY subject")?;
    let rows = stmt.query_map([], |r| Ok((parsed::<Subject>(r.get(0)?)?, parsed(r.get(1)?)?)))?;
    let all: Vec<(Subject, Timestamp)> = rows.collect::<rusqlite::Result<_>>()?;
    Ok(all
        .into_iter()
        .filter_map(|(s, t)| match s {
            Subject::Workload(k) => Some((k, t)),
            _ => None,
        })
        .collect())
}

/// OOM kills in `[from, to]`, as the judge expects them.
pub fn oom_kills(
    c: &Connection,
    host: &str,
    from: Timestamp,
    to: Timestamp,
) -> rusqlite::Result<Vec<LocalEvent>> {
    let mut stmt = c.prepare(
        "SELECT ts, subject FROM events WHERE host = ?1 AND kind = 'oom_killed' AND ts >= ?2 AND ts <= ?3",
    )?;
    let rows = stmt.query_map(params![host, ts(from), ts(to)], |r| {
        let workload = match parsed(r.get(1)?)? {
            Subject::Workload(k) => Some(k),
            _ => None,
        };
        Ok(LocalEvent::OomKilled { ts: parsed(r.get(0)?)?, workload })
    })?;
    rows.collect()
}

pub fn insert_exceptions(
    c: &Connection,
    host: &str,
    report_ts: Timestamp,
    groups: &[ExceptionGroup],
) -> rusqlite::Result<()> {
    let mut stmt = c.prepare(
        "INSERT INTO exception_groups (host, project, service, class, component, code, report_ts, count,
         final_count, first_seen, last_seen, biz_keys_json, sample_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
    )?;
    for g in groups {
        stmt.execute(params![
            host,
            g.workload.project,
            g.workload.service,
            g.class.as_str(),
            g.component,
            g.code,
            ts(report_ts),
            g.count,
            g.final_count,
            ts(g.first_seen),
            ts(g.last_seen),
            json(&g.biz_keys),
            g.sample.as_ref().map(json)
        ])?;
    }
    Ok(())
}

/// Exception groups reported in `(from, to]`, summed by [`sum`]. Optionally one workload or class.
pub fn exceptions(
    c: &Connection,
    host: &str,
    window: (Timestamp, Timestamp),
    workload: Option<&WorkloadKey>,
    class: Option<ExceptionClass>,
) -> rusqlite::Result<Vec<ExceptionGroup>> {
    let mut stmt = c.prepare(
        "SELECT project, service, class, component, code, count, final_count, first_seen, last_seen,
                biz_keys_json, sample_json
         FROM exception_groups
         WHERE host = ?1 AND report_ts > ?2 AND report_ts <= ?3
           AND (?4 IS NULL OR project = ?4) AND (?5 IS NULL OR service = ?5) AND (?6 IS NULL OR class = ?6)
         ORDER BY report_ts DESC",
    )?;
    let args = params![
        host,
        ts(window.0),
        ts(window.1),
        workload.map(|w| &w.project),
        workload.map(|w| &w.service),
        class.map(ExceptionClass::as_str)
    ];
    let rows = stmt.query_map(args, |r| {
        Ok(ExceptionGroup {
            workload: WorkloadKey {
                host: host.to_string(),
                project: r.get(0)?,
                service: r.get(1)?,
            },
            class: parsed(r.get(2)?)?,
            component: r.get(3)?,
            code: r.get(4)?,
            count: r.get(5)?,
            final_count: r.get(6)?,
            first_seen: parsed(r.get(7)?)?,
            last_seen: parsed(r.get(8)?)?,
            biz_keys: from_json(&r.get::<_, String>(9)?)?,
            sample: r.get::<_, Option<String>>(10)?.map(|s| from_json(&s)).transpose()?,
        })
    })?;
    Ok(sum(rows.collect::<rusqlite::Result<_>>()?))
}

/// One group per workload, class, component and code: counts add up, times widen, and the
/// newest group (first, as `newest_first` is ordered) supplies business keys and sample.
pub fn sum(newest_first: Vec<ExceptionGroup>) -> Vec<ExceptionGroup> {
    let key = |g: &ExceptionGroup| {
        (g.workload.clone(), g.class.as_str(), g.component.clone(), g.code.clone())
    };
    let summed = newest_first.into_iter().fold(BTreeMap::new(), |mut acc, g| {
        acc.entry(key(&g))
            .and_modify(|kept: &mut ExceptionGroup| {
                kept.count = kept.count.saturating_add(g.count);
                kept.final_count = kept.final_count.saturating_add(g.final_count);
                kept.first_seen = kept.first_seen.min(g.first_seen);
                kept.last_seen = kept.last_seen.max(g.last_seen);
            })
            .or_insert(g);
        acc
    });
    let mut groups: Vec<ExceptionGroup> = summed.into_values().collect();
    groups.sort_by_key(|g| std::cmp::Reverse(g.last_seen));
    groups
}

pub fn prune(
    c: &Connection,
    events_before: Timestamp,
    exceptions_before: Timestamp,
) -> rusqlite::Result<()> {
    c.execute("DELETE FROM events WHERE ts < ?1", [ts(events_before)])?;
    c.execute("DELETE FROM exception_groups WHERE report_ts < ?1", [ts(exceptions_before)])
        .map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sums_per_group_keeping_the_newest_details() {
        let at = |m: i64| Timestamp::from_second(1_790_000_000 + m * 60).unwrap();
        let group = |code: &str, count, first, last, key: &str| ExceptionGroup {
            workload: WorkloadKey {
                host: "x".into(),
                project: "app".into(),
                service: "api".into(),
            },
            class: ExceptionClass::Application,
            component: "jobs".into(),
            code: code.into(),
            count,
            final_count: count - 1,
            first_seen: at(first),
            last_seen: at(last),
            biz_keys: vec![key.into()],
            sample: None,
        };
        let newest_first =
            vec![group("A", 2, 9, 10, "new"), group("B", 1, 3, 4, "b"), group("A", 3, 1, 5, "old")];
        let summed = sum(newest_first);
        let a = &summed[0];
        assert_eq!((a.code.as_str(), a.count, a.final_count), ("A", 5, 3));
        assert_eq!(
            (a.first_seen, a.last_seen, a.biz_keys.as_slice()),
            (at(1), at(10), &["new".to_string()][..])
        );
        assert_eq!(summed[1].code, "B", "most recent first");
        let other_class =
            ExceptionGroup { class: ExceptionClass::Business, ..group("A", 1, 0, 0, "x") };
        assert_eq!(sum(vec![group("A", 1, 0, 0, "x"), other_class]).len(), 2);
    }
}
