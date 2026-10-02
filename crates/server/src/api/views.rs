//! Stored rows → the JSON views of the query API. Pure.

use crate::config::{Mute, ServerConfig};
use crate::lifecycle::{Change, Incident, State};
use crate::store::incidents::LogEntry;
use jiff::Timestamp;
use skym_core::model::Event;
use skym_core::subject::{HostId, Subject};
use skym_core::time::format_duration;
use skym_core::view::{
    CustomerOverview, HostOverview, IncidentView, Overview, Status, Timeline, TimelineEntry,
    TimelineKind, rollup,
};
use std::collections::BTreeMap;

pub fn incident_view(i: &Incident, mutes: &[Mute], now: Timestamp) -> IncidentView {
    let mute = mutes.iter().find(|m| {
        m.subject == i.subject && m.code == i.code && m.until.is_none_or(|until| now < until)
    });
    IncidentView {
        subject: i.subject.clone(),
        code: i.code,
        severity: i.severity,
        detail: i.detail.clone(),
        opened_at: i.opened_at,
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
        BTreeMap::from([
            ("host".to_string(), format!("/api/hosts/{}", encode(h))),
            ("timeline".to_string(), format!("/api/timeline?host={}&since=6h", encode(h))),
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
            ])
        }
        Subject::Endpoint(_) => BTreeMap::new(),
    }
}

/// Percent-encodes everything but unreserved characters (a replica is `api#2`).
pub fn encode(segment: &str) -> String {
    segment
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// A host that never reported is unknown, and as urgent as a critical one.
pub fn host_status(incidents: &[IncidentView], reported: bool) -> Status {
    if reported { rollup(incidents) } else { Status::Unknown }
}

fn urgency(s: Status) -> u8 {
    match s {
        Status::Critical => 3,
        Status::Unknown => 2,
        Status::Warn => 1,
        Status::Ok => 0,
    }
}

fn worst(statuses: impl Iterator<Item = Status>) -> Status {
    statuses.max_by_key(|s| urgency(*s)).unwrap_or(Status::Ok)
}

/// Customers in configuration order; within each, the most urgent hosts first.
pub fn overview(
    cfg: &ServerConfig,
    last_seen: &BTreeMap<HostId, Timestamp>,
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
                    let seen = last_seen.get(&h.id);
                    HostOverview {
                        id: h.id.clone(),
                        status: host_status(&mine, seen.is_some()),
                        last_report_ago: seen.map(|t| format_duration(now.duration_since(*t))),
                        incidents: mine,
                        links: links(&Subject::Host(h.id.clone())),
                    }
                })
                .collect();
            hosts.sort_by(|a, b| {
                urgency(b.status).cmp(&urgency(a.status)).then_with(|| a.id.cmp(&b.id))
            });
            CustomerOverview {
                id: c.id.clone(),
                name: c.name.clone(),
                status: worst(hosts.iter().map(|h| h.status)),
                hosts,
            }
        })
        .collect();
    Overview {
        ts: now,
        status: worst(customers.iter().map(|c| c.status)),
        muted_count: open.iter().filter(|i| i.muted).count() as u32,
        customers,
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
