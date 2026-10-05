//! Stored rows → the JSON views of the query API. Pure.

use crate::config::{Endpoint, Mute, ServerConfig};
use crate::lifecycle::{Change, Incident, State};
use crate::store::hosts::{HostRow, WorkloadRow};
use crate::store::incidents::LogEntry;
use crate::store::probes::ProbeRow;
use jiff::{SignedDuration, Timestamp};
use skym_core::model::{Event, ExceptionGroup};
use skym_core::rules::{IncidentCode, Severity};
use skym_core::subject::{AppKey, HostId, Subject, WorkloadKey, encode};
use skym_core::time::format_duration;
use skym_core::view::{
    AppSummary, CustomerOverview, DiskUse, EndpointOverview, HostOverview, HostView, IncidentView,
    Overview, Status, Timeline, TimelineEntry, TimelineKind, WorkloadSummary, WorkloadView, rollup,
    workload_summary,
};
use std::collections::BTreeMap;

/// What turns stored incidents into views.
pub struct Context<'a> {
    pub mutes: &'a [Mute],
    /// When stopped workloads stopped, for the real start of `WORKLOAD_DOWN`.
    pub stopped: &'a BTreeMap<WorkloadKey, Timestamp>,
    /// Which application each probed URL belongs to.
    pub probed: &'a [Endpoint],
    /// When skym first heard from each host, and first probed each URL.
    pub hosts_seen: &'a BTreeMap<HostId, Timestamp>,
    pub urls_seen: &'a BTreeMap<String, Timestamp>,
    pub now: Timestamp,
}

pub fn incident_view(i: &Incident, cx: &Context) -> IncidentView {
    let now = cx.now;
    let mute = cx.mutes.iter().find(|m| {
        m.subject == i.subject && m.code == i.code && m.until.is_none_or(|until| now < until)
    });
    let app = app_of(&i.subject, cx.probed);
    let mut links = links(&i.subject);
    if let Some(a) = &app {
        links.insert("app".to_string(), a.path());
    }
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
                cx.stopped.get(k).copied()
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
        links,
        observed_since: match &i.subject {
            Subject::Endpoint(url) => cx.urls_seen.get(url).copied(),
            s => s.host().and_then(|h| cx.hosts_seen.get(h)).copied(),
        },
        app,
        workload: None, // attached where the workloads are known
    }
}

/// The application an incident belongs to; `None` for a host's own problems.
pub fn app_of(subject: &Subject, probed: &[Endpoint]) -> Option<AppKey> {
    match subject {
        Subject::Workload(k) => Some(AppKey::of(k)),
        Subject::App(a) => Some(a.clone()),
        Subject::Endpoint(url) => probed.iter().find(|e| e.url == *url).map(|e| e.app.clone()),
        Subject::Host(_) | Subject::Mount { .. } | Subject::Unknown(_) => None,
    }
}

/// A workload with its links, its status judged by `incidents`.
pub fn summary(w: &WorkloadRow, incidents: &[IncidentView]) -> WorkloadSummary {
    let links = links(&Subject::Workload(w.key.clone()));
    workload_summary(w.key.clone(), w.facts.as_ref(), &w.state, incidents, links)
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

/// One host as stored; `row` is `None` until it first reports. `apps` are its applications.
pub fn host(
    id: HostId,
    row: Option<HostRow>,
    workloads: Vec<WorkloadRow>,
    incidents: Vec<IncidentView>,
    apps: Vec<AppSummary>,
    now: Timestamp,
) -> HostView {
    let workloads = workloads.into_iter().map(|w| summary(&w, &incidents)).collect();
    HostView {
        status: status(&incidents, row.is_some()),
        last_report_ago: row.as_ref().map(|r| format_duration(now.duration_since(r.last_seen))),
        observed_since: row.as_ref().map(|r| r.first_seen),
        facts: row.as_ref().and_then(|r| r.facts.clone()),
        errors: row.as_ref().map(|r| r.errors.clone()).unwrap_or_default(),
        ip: row.as_ref().and_then(|r| r.remote_addr.clone()),
        state: row.map(|r| r.state),
        id,
        customer: None, // customers are gone; the field stays for older readers
        workloads,
        incidents,
        apps,
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

/// One host in a list: how it is doing, what it has left, and its applications. `row` is
/// `None` until it first reports; `open` holds the open incidents, muted ones included.
pub fn host_overview(
    id: &HostId,
    tags: &[String],
    row: Option<&HostRow>,
    apps: &[AppSummary],
    open: &[IncidentView],
    now: Timestamp,
) -> HostOverview {
    let mut tags = tags.to_vec();
    tags.sort();
    tags.dedup();
    let mine: Vec<IncidentView> =
        open.iter().filter(|i| !i.muted && i.subject.host() == Some(id)).cloned().collect();
    let filling = |path: &str| {
        mine.iter().any(|i| {
            i.code == IncidentCode::DiskFilling
                && matches!(&i.subject, Subject::Mount { path: p, .. } if p == path)
        })
    };
    let facts = row.and_then(|r| r.facts.as_ref());
    let fs_type = |path: &str| {
        facts.and_then(|f| f.mounts.iter().find(|m| m.path == path)).map(|m| m.fs_type.clone())
    };
    let percent = |part: u64, whole: u64| (part * 100).checked_div(whole).unwrap_or(0) as u8;
    let disks = row.map_or_else(Vec::new, |r| {
        r.state
            .mounts
            .iter()
            .map(|m| DiskUse {
                path: m.path.clone(),
                used_percent: percent(m.used_bytes, m.total_bytes),
                filling: filling(&m.path),
                total_bytes: m.total_bytes,
                free_bytes: m.total_bytes.saturating_sub(m.used_bytes),
                inodes_percent: percent(m.inodes_used, m.inodes_total),
                fs_type: fs_type(&m.path),
            })
            .collect()
    });
    let own: Vec<&AppSummary> = apps.iter().filter(|a| a.key.host == *id).collect();
    HostOverview {
        id: id.clone(),
        status: status(&mine, row.is_some()),
        last_report_ago: row.map(|r| format_duration(now.duration_since(r.last_seen))),
        observed_since: row.map(|r| r.first_seen),
        info_count: mine.iter().filter(|i| i.severity == Severity::Info).count() as u32,
        links: links(&Subject::Host(id.clone())),
        load_1m: row.map(|r| r.state.load_1m),
        memory_used_bytes: row.map(|r| r.state.memory_used_bytes),
        memory_total_bytes: facts.map(|f| f.memory_total_bytes),
        cpu_percent: row.and_then(|r| r.state.cpu_percent),
        iowait_percent: row.and_then(|r| r.state.iowait_percent),
        steal_percent: row.and_then(|r| r.state.steal_percent),
        net_rx_bytes_per_s: row.and_then(|r| r.state.net_rx_bytes_per_s),
        net_tx_bytes_per_s: row.and_then(|r| r.state.net_tx_bytes_per_s),
        disks,
        apps: own.len() as u32,
        apps_in_trouble: own.iter().filter(|a| a.status != Status::Ok).count() as u32,
        incidents: mine,
        os: facts.map(|f| f.os.clone()),
        kernel: facts.map(|f| f.kernel.clone()),
        arch: facts.map(|f| f.arch.clone()),
        cpu_count: facts.map(|f| f.cpu_count),
        boot_time: facts.map(|f| f.boot_time),
        docker_version: facts.and_then(|f| f.docker_version.clone()),
        agent_version: facts.map(|f| f.agent_version.clone()),
        ip: row.and_then(|r| r.remote_addr.clone()),
        tags,
    }
}

/// Every configured host, most urgent first.
pub fn hosts(
    cfg: &ServerConfig,
    rows: &[HostRow],
    apps: &[AppSummary],
    open: &[IncidentView],
    now: Timestamp,
) -> Vec<HostOverview> {
    let mut hosts: Vec<HostOverview> = cfg
        .hosts
        .iter()
        .map(|h| host_overview(&h.id, &h.tags, rows.iter().find(|r| r.id == h.id), apps, open, now))
        .collect();
    hosts.sort_by(|a, b| b.status.cmp(&a.status).then_with(|| a.id.cmp(&b.id)));
    hosts
}

/// Where something is wrong: every open, unmuted problem, worst and oldest first, and every
/// host. `customers` groups the hosts for views older than this.
pub fn overview(
    cfg: &ServerConfig,
    rows: &[HostRow],
    apps: &[AppSummary],
    open: &[IncidentView],
    now: Timestamp,
) -> Overview {
    let hosts = hosts(cfg, rows, apps, open, now);
    let mut problems: Vec<IncidentView> = open.iter().filter(|i| !i.muted).cloned().collect();
    problems
        .sort_by(|a, b| b.severity.cmp(&a.severity).then_with(|| a.opened_at.cmp(&b.opened_at)));
    let customers = cfg
        .customers
        .iter()
        .map(|c| {
            let theirs: Vec<HostOverview> = hosts
                .iter()
                .filter(|h| {
                    cfg.hosts.iter().any(|e| e.id == h.id && e.customer.as_ref() == Some(&c.id))
                })
                .cloned()
                .collect();
            CustomerOverview {
                id: c.id.clone(),
                name: c.name.clone(),
                status: theirs.iter().map(|h| h.status).max().unwrap_or(Status::Ok),
                hosts: theirs,
                endpoints: Vec::new(),
            }
        })
        .collect();
    let worst = hosts.iter().map(|h| h.status).chain(std::iter::once(rollup(&problems)));
    Overview {
        ts: now,
        status: worst.max().unwrap_or(Status::Ok),
        muted_count: open.iter().filter(|i| i.muted).count() as u32,
        customers,
        problems,
        hosts,
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
        app: Some(e.app.clone()),
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
