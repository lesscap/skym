//! Docker container events → crash restarts and OOM kills. Pure; Docker keeps only the last
//! 256 events, so OOM kills are also taken from the containers' last exit.

use super::containers::workload_key;
use bollard::models::EventMessage;
use jiff::{SignedDuration, Timestamp};
use skym_core::model::LocalEvent;
use skym_core::subject::WorkloadKey;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub struct ContainerEvent {
    pub ts: Timestamp,
    pub id: String,
    pub key: WorkloadKey,
    pub action: Action,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Die { exit_code: i64 },
    Kill,
    Oom,
}

/// Event attributes carry the container name and labels, so recreated containers still map.
pub fn container_event(host: &str, m: &EventMessage) -> Option<ContainerEvent> {
    let actor = m.actor.as_ref()?;
    let attrs = actor.attributes.as_ref()?;
    let action = match m.action.as_deref()? {
        "die" => Action::Die { exit_code: attrs.get("exitCode")?.parse().ok()? },
        "kill" | "stop" => Action::Kill,
        "oom" => Action::Oom,
        _ => return None,
    };
    let ts = match (m.time_nano, m.time) {
        (Some(ns), _) => Timestamp::from_nanosecond(i128::from(ns)).ok()?,
        (None, Some(s)) => Timestamp::from_second(s).ok()?,
        _ => return None,
    };
    let key = workload_key(host, attrs.get("name")?, attrs);
    Some(ContainerEvent { ts, id: actor.id.clone()?, key, action })
}

/// Non-zero exits not preceded by a `kill`/`stop` of the same container within 15 s.
pub fn crash_restarts(events: &[ContainerEvent]) -> BTreeMap<WorkloadKey, Vec<Timestamp>> {
    let killed_before = |e: &ContainerEvent| {
        events.iter().any(|k| {
            k.id == e.id
                && k.action == Action::Kill
                && k.ts <= e.ts
                && e.ts - SignedDuration::from_secs(15) <= k.ts
        })
    };
    events
        .iter()
        .filter(|e| matches!(e.action, Action::Die { exit_code } if exit_code != 0))
        .filter(|e| !killed_before(e))
        .fold(BTreeMap::new(), |mut acc, e| {
            acc.entry(e.key.clone()).or_insert_with(Vec::new).push(e.ts);
            acc
        })
}

/// `oom` events, plus containers whose last exit was an OOM kill within the window
/// (events can fall out of Docker's small buffer).
pub fn ooms(
    events: &[ContainerEvent],
    oom_exits: &[(WorkloadKey, Timestamp)],
    since: Timestamp,
) -> Vec<LocalEvent> {
    let from_events: Vec<(WorkloadKey, Timestamp)> =
        events.iter().filter(|e| e.action == Action::Oom).map(|e| (e.key.clone(), e.ts)).collect();
    let missed = oom_exits
        .iter()
        .filter(|(k, ts)| *ts >= since && !from_events.iter().any(|(ek, _)| ek == k))
        .cloned();
    from_events
        .iter()
        .cloned()
        .chain(missed)
        .map(|(key, ts)| LocalEvent::OomKilled { ts, workload: Some(key) })
        .collect()
}

#[cfg(test)]
#[path = "events_tests.rs"]
mod tests;
