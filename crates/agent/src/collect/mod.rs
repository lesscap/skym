//! One collection pass over the host. A failing source is recorded in `errors`
//! and contributes nothing; the other sources still run.

pub mod containers;
mod datastore;
mod docker;
mod events;
pub mod host;
mod logs;
mod systemd;

use crate::config::Config;
use crate::exceptions::Detail;
use bollard::Docker;
use bollard::models::ContainerInspectResponse;
use containers::{facts_of, state_of, timestamp, winners};
use events::{container_event, crash_restarts, ooms};
use futures_util::future::join_all;
use jiff::Timestamp;
use skym_core::model::{
    DatastoreKind, ExceptionGroup, HostFacts, HostState, LocalEvent, RunState, TransientCounts,
    WorkloadFacts, WorkloadState,
};
use skym_core::subject::WorkloadKey;
use std::collections::BTreeMap;

/// Every success and every failure of independent calls: one failure never hides the rest.
fn split<T, E>(results: impl IntoIterator<Item = Result<T, E>>) -> (Vec<T>, Vec<E>) {
    results.into_iter().fold((Vec::new(), Vec::new()), |(mut ok, mut errors), r| {
        match r {
            Ok(v) => ok.push(v),
            Err(e) => errors.push(e),
        }
        (ok, errors)
    })
}

pub struct Window<'a> {
    pub now: Timestamp,
    /// Container events and OOM exits from here on.
    pub events_since: Timestamp,
    /// Log lines strictly after the workload's cursor, or after this without one.
    pub logs_after: Timestamp,
    pub cursors: &'a BTreeMap<WorkloadKey, Timestamp>,
    pub detail: Detail,
}

pub struct Workload {
    pub key: WorkloadKey,
    pub facts: WorkloadFacts,
    pub state: WorkloadState,
}

#[derive(Default)]
pub struct Collected {
    pub host: Option<(HostFacts, HostState)>,
    pub workloads: Vec<Workload>,
    /// Containers that are not workloads, counted.
    pub transient: TransientCounts,
    pub local_events: Vec<LocalEvent>,
    pub exceptions: Vec<ExceptionGroup>,
    pub errors: Vec<String>,
    /// Every enabled source failed: nothing here is worth judging.
    pub all_failed: bool,
    /// The time of the last log line read, per workload that had any.
    pub log_ends: BTreeMap<WorkloadKey, Timestamp>,
    /// The kernel's `oom_kill` counter (`/proc/vmstat`), when readable.
    pub oom_kill_count: Option<u64>,
    /// systemd's `NRestarts` per unit.
    pub unit_restart_counts: BTreeMap<WorkloadKey, u32>,
    /// Docker's events were read up to `Window::now`.
    pub events_read: bool,
    /// Every container was listed and inspected: a workload missing here is gone.
    pub workloads_complete: bool,
    /// Docker is enabled and listed every container (what the server needs to call one gone).
    pub containers_listed: bool,
}

pub async fn collect(cfg: &Config, host: &str, w: &Window<'_>) -> Collected {
    let mut errors = Vec::new();
    let docker = match cfg.docker.enabled {
        true => note(&mut errors, "docker", connect(cfg).await),
        false => None,
    };
    let daemon = docker.as_ref().map(|(_, daemon)| daemon);
    let root = daemon.and_then(|d| d.root_dir.as_deref());
    let host_read = host::read(host, root, daemon.and_then(|d| d.version.clone()));
    let host_info = note(&mut errors, "host", host_read).map(|(facts, state, mount_errors)| {
        errors.extend(mount_errors);
        (facts, state)
    });
    let from_docker = match &docker {
        Some((d, _)) => note(&mut errors, "docker", from_docker(d, cfg, host, w).await),
        None => None,
    };
    let oom_kill_count =
        std::fs::read_to_string("/proc/vmstat").ok().and_then(|s| host::oom_kills(&s));
    let (units, unit_errors) = systemd::collect(host, &cfg.systemd).await;
    let outcomes = [
        Some(host_info.is_some()),
        cfg.docker.enabled.then_some(from_docker.is_some()),
        (!cfg.systemd.is_empty()).then_some(!units.is_empty()),
    ];
    let out = from_docker.unwrap_or_default();
    errors.extend(out.errors.into_iter().chain(unit_errors));
    let unit_restart_counts =
        units.iter().filter_map(|u| Some((u.key.clone(), u.restart_count?))).collect();
    let units = units.into_iter().map(|u| Workload { key: u.key, facts: u.facts, state: u.state });
    Collected {
        host: host_info,
        workloads: out.workloads.into_iter().chain(units).collect(),
        transient: out.transient,
        local_events: out.ooms,
        exceptions: out.exceptions,
        errors,
        all_failed: outcomes.iter().flatten().all(|ok| !ok),
        log_ends: out.log_ends,
        oom_kill_count,
        unit_restart_counts,
        events_read: out.events_read,
        workloads_complete: !cfg.docker.enabled || out.complete,
        containers_listed: cfg.docker.enabled && out.complete,
    }
}

/// One pass for a local command: events from the last hour, logs after `logs_after`, no
/// cursors. Collection errors go to stderr. Returns the hostname used.
pub async fn once(
    cfg: &Config,
    now: Timestamp,
    logs_after: Timestamp,
    detail: Detail,
) -> (String, Collected) {
    let host = host::hostname();
    let events_since = now - jiff::SignedDuration::from_hours(1);
    let cursors = BTreeMap::new();
    let window = Window { now, events_since, logs_after, cursors: &cursors, detail };
    let collected = collect(cfg, &host, &window).await;
    collected.errors.iter().for_each(|e| eprintln!("error: {e}"));
    (host, collected)
}

/// The value, or `None` with the failure recorded.
fn note<T>(errors: &mut Vec<String>, source: &str, result: anyhow::Result<T>) -> Option<T> {
    result.map_err(|e| errors.push(format!("{source}: {e:#}"))).ok()
}

async fn connect(cfg: &Config) -> anyhow::Result<(Docker, docker::Daemon)> {
    let d = docker::connect(&cfg.docker.socket).await?;
    let daemon = docker::daemon(&d).await?;
    Ok((d, daemon))
}

#[derive(Default)]
struct DockerOutput {
    workloads: Vec<Workload>,
    transient: TransientCounts,
    ooms: Vec<LocalEvent>,
    exceptions: Vec<ExceptionGroup>,
    errors: Vec<String>,
    log_ends: BTreeMap<WorkloadKey, Timestamp>,
    events_read: bool,
    complete: bool,
}

async fn from_docker(
    d: &Docker,
    cfg: &Config,
    host: &str,
    w: &Window<'_>,
) -> anyhow::Result<DockerOutput> {
    let (inspects, mut errors) = docker::inspect_all(d, &cfg.docker.exclude).await?;
    let complete = errors.is_empty();
    let (events, events_read): (Vec<_>, bool) = match docker::events(d, w.events_since, w.now).await
    {
        Ok(messages) => (messages.iter().filter_map(|m| container_event(host, m)).collect(), true),
        Err(e) => {
            errors.push(format!("{e:#}"));
            (Vec::new(), false)
        }
    };
    let mut restarts = crash_restarts(&events);
    let (chosen, transient) = winners(host, &inspects);
    let oom_exits: Vec<(WorkloadKey, Timestamp)> =
        chosen.iter().filter_map(|(key, i)| Some((key.clone(), oom_exit(i)?))).collect();
    let workloads: Vec<_> =
        chosen.into_iter().map(|(key, i)| (workload(key, i, &mut restarts), i)).collect();
    let probed = join_all(workloads.into_iter().map(probe)).await;
    let targets: Vec<(WorkloadKey, String, Timestamp)> = probed
        .iter()
        .filter(|(wl, _)| wl.state.run != RunState::Created)
        .map(|(wl, id)| {
            let after = w.cursors.get(&wl.key).copied().unwrap_or(w.logs_after);
            (wl.key.clone(), id.clone(), after)
        })
        .collect();
    let read = logs::read(d, &targets, w.detail).await;
    errors.extend(read.errors);
    Ok(DockerOutput {
        workloads: probed.into_iter().map(|(wl, _)| wl).collect(),
        transient,
        ooms: ooms(&events, &oom_exits, w.events_since),
        exceptions: read.groups,
        errors,
        log_ends: read.ends,
        events_read,
        complete,
    })
}

fn oom_exit(i: &ContainerInspectResponse) -> Option<Timestamp> {
    let state = i.state.as_ref().filter(|s| s.oom_killed == Some(true))?;
    timestamp(state.finished_at.as_deref()?)
}

/// The chosen container's facts and state, with its crash restarts and memory use.
fn workload(
    key: WorkloadKey,
    i: &ContainerInspectResponse,
    restarts: &mut BTreeMap<WorkloadKey, Vec<Timestamp>>,
) -> Workload {
    let running = i.state.as_ref().and_then(|s| s.running) == Some(true);
    let memory = i.id.as_deref().filter(|_| running).and_then(docker::memory);
    let state = state_of(i, restarts.remove(&key).unwrap_or_default(), memory);
    Workload { key, facts: facts_of(i), state }
}

/// Datastores get a protocol probe; returns the workload with its container id.
async fn probe((mut wl, i): (Workload, &ContainerInspectResponse)) -> (Workload, String) {
    if let (Some(kind), RunState::Running) = (wl.facts.datastore, wl.state.run) {
        let networks = i.network_settings.as_ref().and_then(|n| n.networks.as_ref());
        let ip =
            networks.into_iter().flatten().find_map(|(_, e)| e.ip_address.as_ref()?.parse().ok());
        let host_mode =
            i.host_config.as_ref().and_then(|h| h.network_mode.as_deref()) == Some("host");
        let ip = ip.or(host_mode.then(|| [127, 0, 0, 1].into()));
        let env = i.config.as_ref().and_then(|c| c.env.as_deref()).unwrap_or_default();
        let creds = (kind == DatastoreKind::Postgres).then(|| datastore::pg_creds(env));
        wl.state.datastore = Some(datastore::probe(kind, ip, creds.as_ref()).await);
    }
    (wl, i.id.clone().unwrap_or_default())
}
