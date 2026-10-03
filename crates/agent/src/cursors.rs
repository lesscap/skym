//! Where log reading stopped, per workload: the time of the last line read. Kept in
//! `state_dir/cursors.json`, so a restart of `skym` neither repeats nor skips lines.

use anyhow::Context;
use jiff::{SignedDuration, Timestamp};
use skym_core::subject::WorkloadKey;
use std::collections::BTreeMap;
use std::path::Path;

/// Logs older than this are not read: a long outage of `skym` is not caught up on.
const MAX_BACKLOG: SignedDuration = SignedDuration::from_hours(1);

#[derive(Default, Debug, PartialEq)]
pub struct Cursors(BTreeMap<WorkloadKey, Timestamp>);

impl Cursors {
    /// A missing file means no cursors yet; an unreadable or damaged one is an error.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let text = match std::fs::read_to_string(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Cursors::default()),
            read => read.with_context(|| format!("cannot read {}", path.display()))?,
        };
        let pairs: Vec<(WorkloadKey, Timestamp)> =
            serde_json::from_str(&text).with_context(|| format!("damaged {}", path.display()))?;
        Ok(Cursors(pairs.into_iter().collect()))
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        let pairs: Vec<(&WorkloadKey, &Timestamp)> = self.0.iter().collect();
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec(&pairs)?)
            .with_context(|| format!("cannot write {}", tmp.display()))?;
        std::fs::rename(&tmp, path).with_context(|| format!("cannot replace {}", path.display()))
    }

    /// Where each workload's reading starts this pass.
    pub fn starts(&self, now: Timestamp) -> BTreeMap<WorkloadKey, Timestamp> {
        self.0.iter().map(|(k, t)| (k.clone(), (*t).max(now - MAX_BACKLOG))).collect()
    }

    /// Moves on the workloads that read lines. Called only once the report holding those
    /// lines is safely stored.
    pub fn advance(&mut self, ends: &BTreeMap<WorkloadKey, Timestamp>) {
        self.0.extend(ends.iter().map(|(k, t)| (k.clone(), *t)));
    }

    /// Forgets workloads that are gone; only after a pass in which every source worked.
    pub fn keep_only<'a>(&mut self, present: impl IntoIterator<Item = &'a WorkloadKey>) {
        let present: Vec<&WorkloadKey> = present.into_iter().collect();
        self.0.retain(|k, _| present.contains(&k));
    }
}

/// Where reading starts for a workload without a cursor (it never logged): where the last
/// pass read from, else `fallback`; never more than an hour back.
pub fn default_start(
    last_read: Option<Timestamp>,
    fallback: Timestamp,
    now: Timestamp,
) -> Timestamp {
    last_read.unwrap_or(fallback).max(now - MAX_BACKLOG)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(service: &str) -> WorkloadKey {
        WorkloadKey { host: "x".into(), project: "app".into(), service: service.into() }
    }

    fn t(sec: i64) -> Timestamp {
        Timestamp::from_second(1_790_000_000 + sec).unwrap()
    }

    #[test]
    fn cursors_move_with_lines_read_and_drop_gone_workloads() {
        let mut c = Cursors::default();
        c.advance(&BTreeMap::from([(key("api"), t(10)), (key("web"), t(20))]));
        // a pass where api read nothing (or failed) and web is gone
        c.advance(&BTreeMap::new());
        c.keep_only([&key("api")]);
        assert_eq!(c.starts(t(60)), BTreeMap::from([(key("api"), t(10))]));
        c.advance(&BTreeMap::from([(key("api"), t(70))]));
        assert_eq!(c.starts(t(80))[&key("api")], t(70));
    }

    #[test]
    fn a_quiet_workload_reads_from_the_last_pass() {
        assert_eq!(default_start(Some(t(100)), t(150), t(160)), t(100));
        assert_eq!(default_start(None, t(150), t(160)), t(150));
        assert_eq!(default_start(Some(t(0)), t(7190), t(7200)), t(3600));
    }

    #[test]
    fn reading_never_starts_more_than_an_hour_back() {
        let mut c = Cursors::default();
        c.advance(&BTreeMap::from([(key("api"), t(0))]));
        assert_eq!(c.starts(t(7200))[&key("api")], t(3600));
    }

    #[test]
    fn cursors_survive_a_restart() {
        let dir = std::env::temp_dir().join(format!("skym-cursors-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("cursors.json");
        assert_eq!(Cursors::load(&path).unwrap(), Cursors::default(), "no file yet");
        let mut c = Cursors::default();
        c.advance(&BTreeMap::from([(key("api"), t(5))]));
        c.save(&path).unwrap();
        assert_eq!(Cursors::load(&path).unwrap(), c);
        std::fs::write(&path, "not json").unwrap();
        assert!(Cursors::load(&path).is_err(), "damaged");
        assert!(Cursors::load(&dir).is_err(), "unreadable is not missing");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
