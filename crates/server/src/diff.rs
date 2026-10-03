//! Events from a report: facts compared with the stored ones, plus the restarts and OOM
//! kills the host saw. Pure; the store makes them idempotent.

use serde_json::Value;
use skym_core::model::{Event, EventKind, HostFacts, LocalEvent, WorkloadFacts};
use skym_core::report::Report;
use skym_core::subject::{Subject, WorkloadKey};
use std::collections::BTreeMap;

/// A workload's last stored facts, as raw JSON, and the agent version that reported them.
pub struct Stored {
    pub facts: Value,
    pub agent: Option<String>,
}

pub fn events(
    prev_host: Option<&HostFacts>,
    stored: &BTreeMap<WorkloadKey, Stored>,
    r: &Report,
) -> Vec<Event> {
    let host = Subject::Host(r.host.clone());
    let event = |subject: &Subject, kind| Event { ts: r.ts, subject: subject.clone(), kind };
    let mut out = Vec::new();
    if let (Some(old), Some(new)) = (prev_host, r.host_facts.as_ref()) {
        if old.boot_time != new.boot_time {
            out.push(event(&host, EventKind::HostRebooted));
        }
        if old.kernel != new.kernel {
            out.push(event(&host, EventKind::KernelChanged));
        }
    }
    let agent = reporting_agent(r);
    for w in &r.workloads {
        let subject = Subject::Workload(w.key.clone());
        let change = w
            .facts
            .as_ref()
            .zip(stored.get(&w.key))
            .and_then(|(new, old)| fact_change(old, new, old.agent.as_deref() == agent));
        out.extend(change.map(|kind| event(&subject, kind)));
        out.extend(w.state.restarts.iter().map(|ts| Event {
            ts: *ts,
            subject: subject.clone(),
            kind: EventKind::Restarted,
        }));
    }
    out.extend(r.local_events.iter().filter_map(|e| match e {
        LocalEvent::OomKilled { ts, workload } => Some(Event {
            ts: *ts,
            subject: workload.clone().map_or_else(|| host.clone(), Subject::Workload),
            kind: EventKind::OomKilled,
        }),
        LocalEvent::Unknown => None,
    }));
    out
}

/// The version of the `skym` that sent the report.
pub fn reporting_agent(r: &Report) -> Option<&str> {
    r.agent_version.as_deref().or(r.host_facts.as_ref().map(|f| f.agent_version.as_str()))
}

/// A new image or image ID is a deployment. Any other difference in a field both sides have
/// is a configuration change, but only between facts from the same agent version: another
/// version may know more fields or compute them differently.
fn fact_change(old: &Stored, new: &WorkloadFacts, same_agent: bool) -> Option<EventKind> {
    let (old, new) = (&old.facts, serde_json::to_value(new).ok()?);
    let field =
        |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
    let (old_image, new_image) = (field(old, "image"), field(&new, "image"));
    let (old_id, new_id) = (field(old, "image_digest"), field(&new, "image_digest"));
    if old_image != new_image || old_id != new_id {
        let short =
            |id: &str| id.trim_start_matches("sha256:").chars().take(12).collect::<String>();
        let (from, to) = match old_image != new_image {
            true => (old_image, new_image),
            false => (short(&old_id), short(&new_id)),
        };
        return Some(EventKind::Deployed { from, to });
    }
    let (Some(old), Some(new)) = (old.as_object(), new.as_object()) else { return None };
    // Recreating a container with the same configuration only moves `created`.
    let changed = old.iter().any(|(k, v)| k != "created" && new.get(k).is_some_and(|n| n != v));
    (changed && same_agent).then_some(EventKind::ConfigChanged)
}

#[cfg(test)]
#[path = "diff_tests.rs"]
mod tests;
