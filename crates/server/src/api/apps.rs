//! Applications: workloads grouped by application, described by the configuration. Pure.

use super::views::status;
use crate::config::AppConfig;
use crate::store::hosts::WorkloadRow;
use jiff::Timestamp;
use skym_core::model::RunState;
use skym_core::rules::IncidentCode;
use skym_core::subject::{AppKey, Subject, WorkloadKey, encode};
use skym_core::view::{AppSummary, EndpointOverview, IncidentView};
use std::collections::BTreeMap;

/// Every application: those with workloads, and configured ones without any. Most urgent
/// first. `open` holds open incidents, muted ones are left out here; `endpoints` are the
/// probes of applications.
pub fn summaries(
    workloads: &[WorkloadRow],
    configured: &[AppConfig],
    endpoints: &[EndpointOverview],
    open: &[IncidentView],
    deployed: &BTreeMap<WorkloadKey, Timestamp>,
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
            summary(key, &rows, config, endpoints, open, deployed)
        })
        .collect();
    apps.sort_by(|a, b| b.status.cmp(&a.status).then_with(|| a.key.cmp(&b.key)));
    apps
}

fn summary(
    key: AppKey,
    rows: &[&WorkloadRow],
    config: Option<&AppConfig>,
    endpoints: &[EndpointOverview],
    open: &[IncidentView],
    deployed: &BTreeMap<WorkloadKey, Timestamp>,
) -> AppSummary {
    let endpoints: Vec<EndpointOverview> =
        endpoints.iter().filter(|e| e.app.as_ref() == Some(&key)).cloned().collect();
    let incidents: Vec<IncidentView> =
        open.iter().filter(|i| !i.muted && belongs(i, &key, &endpoints)).cloned().collect();
    AppSummary {
        name: config.and_then(|c| c.name.clone()).unwrap_or_else(|| key.label().to_string()),
        env: config.and_then(|c| c.env.clone()),
        note: config.and_then(|c| c.note.clone()),
        configured: config.is_some(),
        status: status(&incidents, !rows.is_empty() || !incidents.is_empty()),
        services: rows.len() as u32,
        running: rows.iter().filter(|w| w.state.run == RunState::Running).count() as u32,
        last_deployed: rows.iter().filter_map(|w| deployed.get(&w.key)).max().copied(),
        endpoints,
        incidents,
        links: BTreeMap::from([
            ("app".to_string(), key.path()),
            ("host".to_string(), format!("/api/hosts/{}", encode(&key.host))),
        ]),
        key,
    }
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
