//! Applications: workloads grouped by application, described by the configuration. Pure.

use super::views::{self, links, status};
use crate::config::{AppConfig, HostEntry};
use crate::store::hosts::WorkloadRow;
use jiff::Timestamp;
use skym_core::model::{Event, EventKind, RunState};
use skym_core::rules::IncidentCode;
use skym_core::subject::{AppKey, Subject, WorkloadKey};
use skym_core::view::{
    AppSummary, Deploy, EndpointOverview, IncidentView, Status, WorkloadSummary,
};
use std::collections::BTreeMap;

/// What happened to workloads lately: when each was last deployed, recent deployments
/// (newest first) and application exceptions in the last hour.
#[derive(Default)]
pub struct History {
    pub last_deployed: BTreeMap<WorkloadKey, Timestamp>,
    pub deploys: Vec<Event>,
    pub exceptions_1h: BTreeMap<WorkloadKey, u32>,
}

/// How many deployments an application lists.
const DEPLOYS: usize = 10;

/// Every application: those with workloads, and configured ones without any. Most urgent
/// first. `open` holds open incidents, muted ones are left out here; `endpoints` are the
/// probes of applications.
pub fn summaries(
    workloads: &[WorkloadRow],
    hosts: &[HostEntry],
    configured: &[AppConfig],
    endpoints: &[EndpointOverview],
    open: &[IncidentView],
    history: &History,
) -> Vec<AppSummary> {
    let mut keys: BTreeMap<AppKey, Vec<&WorkloadRow>> = BTreeMap::new();
    for w in workloads {
        keys.entry(AppKey::of(&w.key)).or_default().push(w);
    }
    for a in configured {
        keys.entry(a.id.clone()).or_default();
    }
    let mut apps: Vec<AppSummary> = keys
        .into_iter()
        .map(|(key, rows)| {
            let config = configured.iter().find(|a| a.id == key);
            let host = hosts.iter().find(|h| h.id == key.host).map_or(&[][..], |h| &h.tags);
            summary(key, &rows, config, host, endpoints, open, history)
        })
        .collect();
    apps.sort_by(|a, b| b.status.cmp(&a.status).then_with(|| a.key.cmp(&b.key)));
    apps
}

fn summary(
    key: AppKey,
    rows: &[&WorkloadRow],
    config: Option<&AppConfig>,
    host_tags: &[String],
    endpoints: &[EndpointOverview],
    open: &[IncidentView],
    history: &History,
) -> AppSummary {
    let mut tags: Vec<String> =
        host_tags.iter().chain(config.iter().flat_map(|c| &c.tags)).cloned().collect();
    tags.sort();
    tags.dedup();
    let endpoints: Vec<EndpointOverview> =
        endpoints.iter().filter(|e| e.app.as_ref() == Some(&key)).cloned().collect();
    let incidents: Vec<IncidentView> =
        open.iter().filter(|i| !i.muted && belongs(i, &key, &endpoints)).cloned().collect();
    AppSummary {
        name: config.and_then(|c| c.name.clone()).unwrap_or_else(|| key.label().to_string()),
        env: config.and_then(|c| c.env.clone()),
        note: config.and_then(|c| c.note.clone()),
        configured: config.is_some(),
        // Known by its services, its problems, or (an external one) its URLs' answers.
        status: status(
            &incidents,
            !rows.is_empty()
                || !incidents.is_empty()
                || endpoints.iter().any(|e| e.status != Status::Unknown),
        ),
        services: rows.len() as u32,
        running: rows.iter().filter(|w| w.state.run == RunState::Running).count() as u32,
        last_deployed: rows.iter().filter_map(|w| history.last_deployed.get(&w.key)).max().copied(),
        workloads: workload_summaries(rows, &incidents),
        deploys: history
            .deploys
            .iter()
            .filter_map(|e| match (&e.subject, &e.kind) {
                (Subject::Workload(k), EventKind::Deployed { from, to }) if key.contains(k) => {
                    Some(Deploy {
                        ts: e.ts,
                        service: k.service.clone(),
                        from: from.clone(),
                        to: to.clone(),
                    })
                }
                _ => None,
            })
            .take(DEPLOYS)
            .collect(),
        exceptions_1h: rows.iter().filter_map(|w| history.exceptions_1h.get(&w.key)).sum(),
        tags,
        endpoints,
        incidents,
        links: links(&Subject::App(key.clone())),
        key,
    }
}

/// Its workloads with their own status, those with problems first.
fn workload_summaries(rows: &[&WorkloadRow], incidents: &[IncidentView]) -> Vec<WorkloadSummary> {
    let mut list: Vec<WorkloadSummary> =
        rows.iter().map(|w| views::summary(w, incidents)).collect();
    list.sort_by(|a, b| b.status.cmp(&a.status).then_with(|| a.key.cmp(&b.key)));
    list
}

/// An incident of the app's workloads, of the app itself, of one of its probes, or its
/// host's lost heartbeat (nothing about the app is current then).
fn belongs(i: &IncidentView, key: &AppKey, endpoints: &[EndpointOverview]) -> bool {
    match &i.subject {
        Subject::Host(h) => i.code == IncidentCode::HeartbeatLost && *h == key.host,
        Subject::Workload(w) => key.contains(w),
        Subject::App(a) => a == key,
        Subject::Endpoint(url) => endpoints.iter().any(|e| e.url == *url),
        _ => false,
    }
}

#[cfg(test)]
#[path = "apps_tests.rs"]
mod tests;
