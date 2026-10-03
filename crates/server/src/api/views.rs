//! Stored rows → the JSON views of the query API. Pure.

use crate::config::{Endpoint, Mute, ServerConfig};
use crate::lifecycle::{Change, Incident, State};
use crate::store::hosts::{HostRow, WorkloadRow};
use crate::store::incidents::LogEntry;
use crate::store::probes::ProbeRow;
use jiff::{SignedDuration, Timestamp};
use skym_core::model::{Event, ExceptionGroup};
use skym_core::rules::{IncidentCode, Severity};
use skym_core::subject::{AppKey, CustomerId, HostId, Subject, WorkloadKey, encode};
use skym_core::time::format_duration;
use skym_core::view::{
    CustomerOverview, EndpointOverview, HostOverview, HostView, IncidentView, Overview, Status,
    Timeline, TimelineEntry, TimelineKind, WorkloadView, rollup, workload_summary,
};
use std::collections::BTreeMap;

/// `stopped`: when stopped workloads stopped, for the real start of `WORKLOAD_DOWN`.
pub fn incident_view(
    i: &Incident,
    mutes: &[Mute],
    stopped: &BTreeMap<WorkloadKey, Timestamp>,
    now: Timestamp,
) -> IncidentView {
    let mute = mutes.iter().find(|m| {
        m.subject == i.subject && m.code == i.code && m.until.is_none_or(|until| now < until)
    });
    IncidentView {
        subject: i.subject.clone(),
        code: i.code,
        severity: i.severity,
        detail: i.detail.clone(),
        opened_at: i.opened_at,
        since: match &i.subject {
            Subject::Workload(k)
                if i.code == IncidentCode::WorkloadDown && i.state == State::Open =>
            {
                stopped.get(k).copied()
            }
            _ => None,
        },
        open_for: i
            .opened_at
            .filter(|_| i.state == State::Open)
            .map(|t| format_duration(now.duration_since(t))),
        resolved_at: i.resolved_at,
        muted: mute.is_some(),
        mute_reason: mute.and_then(|m| m.reason.clone()),
        links: links(&i.subject),
    }
}

/// Where to look next, so an agent never builds URLs itself.
pub fn links(subject: &Subject) -> BTreeMap<String, String> {
    let host_links = |h: &str| {
        let h = encode(h);
        BTreeMap::from([
            ("host".to_string(), format!("/api/hosts/{h}")),
            ("timeline".to_string(), format!("/api/timeline?host={h}&since=6h")),
            ("exceptions".to_string(), format!("/api/exceptions?host={h}&since=1h")),
            ("incidents".to_string(), format!("/api/incidents?host={h}")),
        ])
    };
    match subject {
        Subject::Host(h) | Subject::Mount { host: h, .. } => host_links(h),
        Subject::Workload(k) => {
            let (h, p, s) = (encode(&k.host), encode(&k.project), encode(&k.service));
            BTreeMap::from([
                ("host".to_string(), format!("/api/hosts/{h}")),
                ("workload".to_string(), format!("/api/hosts/{h}/workloads/{p}/{s}")),
                (
                    "timeline".to_string(),
                    format!("/api/timeline?host={h}&workload={p}/{s}&since=6h"),
                ),
                (
                    "exceptions".to_string(),
                    format!("/api/exceptions?host={h}&workload={p}/{s}&since=1h"),
                ),
                ("incidents".to_string(), format!("/api/incidents?host={h}")),
                ("app".to_string(), AppKey::of(k).path()),
            ])
        }
        Subject::App(a) => BTreeMap::from([
            ("app".to_string(), a.path()),
            ("host".to_string(), format!("/api/hosts/{}", encode(&a.host))),
        ]),
        Subject::Endpoint(_) | Subject::Unknown(_) => BTreeMap::new(),
    }
}

/// A host that never reported, or an endpoint without a recent probe, is unknown.
pub fn status(incidents: &[IncidentView], known: bool) -> Status {
    if known { rollup(incidents) } else { Status::Unknown }
}

/// One host as stored; `row` is `None` until it first reports.
pub fn host(
    id: HostId,
    customer: Option<CustomerId>,
    row: Option<HostRow>,
    workloads: Vec<WorkloadRow>,
    incidents: Vec<IncidentView>,
    now: Timestamp,
) -> HostView {
    let workloads = workloads
        .into_iter()
        .map(|w| {
            let links = links(&Subject::Workload(w.key.clone()));
            workload_summary(w.key, w.facts.as_ref(), &w.state, &incidents, links)
        })
        .collect();
    HostView {
        status: status(&incidents, row.is_some()),
        last_report_ago: row.as_ref().map(|r| format_duration(now.duration_since(r.last_seen))),
        observed_since: row.as_ref().map(|r| r.first_seen),
        facts: row.as_ref().and_then(|r| r.facts.clone()),
        errors: row.as_ref().map(|r| r.errors.clone()).unwrap_or_default(),
        state: row.map(|r| r.state),
        id,
        customer,
        workloads,
        incidents,
    }
}

/// One workload with its own incidents, recent exceptions and events.
pub fn workload(
    row: WorkloadRow,
    incidents: Vec<IncidentView>,
    exceptions: Vec<ExceptionGroup>,
    events: Vec<Event>,
) -> WorkloadView {
    WorkloadView {
        status: rollup(&incidents),
        links: links(&Subject::Workload(row.key.clone())),
        key: row.key,
        facts: row.facts,
        state: row.state,
        incidents,
        exceptions,
        events,
    }
}

/// When skym first and last heard from a host.
pub struct Seen {
    pub first: Timestamp,
    pub last: Timestamp,
}

/// Customers in configuration order; within each, the most urgent hosts and endpoints first.
pub fn overview(
    cfg: &ServerConfig,
    probed: &[Endpoint],
    seen: &BTreeMap<HostId, Seen>,
    probes: &BTreeMap<String, ProbeRow>,
    open: &[IncidentView],
    now: Timestamp,
) -> Overview {
    let customers: Vec<CustomerOverview> = cfg
        .customers
        .iter()
        .map(|c| {
            let mut hosts: Vec<HostOverview> = cfg
                .hosts
                .iter()
                .filter(|h| h.customer == c.id)
                .map(|h| {
                    let subject_host = |i: &&IncidentView| i.subject.host() == Some(&h.id);
                    let mine: Vec<IncidentView> =
                        open.iter().filter(subject_host).filter(|i| !i.muted).cloned().collect();
                    let seen = seen.get(&h.id);
                    HostOverview {
                        id: h.id.clone(),
                        status: status(&mine, seen.is_some()),
                        last_report_ago: seen.map(|s| format_duration(now.duration_since(s.last))),
                        observed_since: seen.map(|s| s.first),
                        info_count: mine.iter().filter(|i| i.severity == Severity::Info).count()
                            as u32,
                        incidents: mine,
                        links: links(&Subject::Host(h.id.clone())),
                    }
                })
                .collect();
            hosts.sort_by(|a, b| b.status.cmp(&a.status).then_with(|| a.id.cmp(&b.id)));
            let mut endpoints: Vec<EndpointOverview> = probed
                .iter()
                .filter(|e| e.customer == c.id)
                .map(|e| endpoint(e, probes.get(&e.url), open, cfg.report_interval, now))
                .collect();
            endpoints.sort_by(|a, b| b.status.cmp(&a.status).then_with(|| a.url.cmp(&b.url)));
            let statuses = hosts.iter().map(|h| h.status).chain(endpoints.iter().map(|e| e.status));
            CustomerOverview {
                id: c.id.clone(),
                name: c.name.clone(),
                status: statuses.max().unwrap_or(Status::Ok),
                hosts,
                endpoints,
            }
        })
        .collect();
    Overview {
        ts: now,
        status: customers.iter().map(|c| c.status).max().unwrap_or(Status::Ok),
        muted_count: open.iter().filter(|i| i.muted).count() as u32,
        customers,
    }
}

/// One endpoint with its unmuted open incidents; `row` is `None` until it is first probed.
/// A probe older than three intervals says nothing about now: the probes have stopped.
pub fn endpoint(
    e: &Endpoint,
    row: Option<&ProbeRow>,
    open: &[IncidentView],
    interval: SignedDuration,
    now: Timestamp,
) -> EndpointOverview {
    let subject = Subject::Endpoint(e.url.clone());
    let mine: Vec<IncidentView> =
        open.iter().filter(|i| i.subject == subject && !i.muted).cloned().collect();
    let answered = row.filter(|r| r.probe.response.is_ok());
    EndpointOverview {
        url: e.url.clone(),
        status: status(&mine, row.is_some_and(|r| now.duration_since(r.probe.at) <= interval * 3)),
        last_probe_ago: row.map(|r| format_duration(now.duration_since(r.probe.at))),
        observed_since: row.map(|r| r.first_seen),
        http_status: answered.and_then(|r| r.probe.response.clone().ok()),
        latency_ms: answered.map(|r| r.probe.latency_ms),
        cert_expires_at: answered.and_then(|r| r.probe.cert_not_after),
        incidents: mine,
        app: e.app.clone(),
    }
}

/// At most `limit` items, and whether any were cut.
pub fn cap<T>(mut items: Vec<T>, limit: usize) -> (Vec<T>, bool) {
    let truncated = items.len() > limit;
    items.truncate(limit);
    (items, truncated)
}

/// Incident changes and events merged, newest first.
pub fn timeline(changes: Vec<LogEntry>, events: Vec<Event>, limit: usize) -> Timeline {
    let from_changes = changes.into_iter().map(|c| TimelineEntry {
        ts: c.ts,
        subject: c.subject,
        entry: match c.change {
            Change::Opened => TimelineKind::IncidentOpened {
                code: c.code,
                severity: c.severity,
                detail: c.detail,
            },
            Change::Reopened => TimelineKind::IncidentReopened {
                code: c.code,
                severity: c.severity,
                detail: c.detail,
            },
            Change::Resolved => TimelineKind::IncidentResolved { code: c.code },
            Change::Severity => {
                TimelineKind::SeverityChanged { code: c.code, severity: c.severity }
            }
        },
    });
    let from_events = events.into_iter().map(|e| TimelineEntry {
        ts: e.ts,
        subject: e.subject,
        entry: TimelineKind::Event { event: e.kind },
    });
    let mut entries: Vec<TimelineEntry> = from_changes.chain(from_events).collect();
    entries.sort_by_key(|e| std::cmp::Reverse(e.ts));
    let (entries, truncated) = cap(entries, limit);
    Timeline { entries, truncated }
}

#[cfg(test)]
#[path = "views_tests.rs"]
mod tests;
