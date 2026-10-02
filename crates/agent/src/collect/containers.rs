//! Docker containers → workloads. Pure functions over Docker's inspect model;
//! container environment variables are never read here.

use bollard::models::{
    ContainerInspectResponse, ContainerStateStatusEnum, HealthStatusEnum, RestartPolicyNameEnum,
};
use jiff::Timestamp;
use skym_core::model::{
    DatastoreKind, Health, RunState, TransientCounts, WorkloadFacts, WorkloadKind, WorkloadState,
};
use skym_core::subject::WorkloadKey;
use std::collections::{BTreeMap, HashMap};

const COMPOSE: &str = "com.docker.compose.";
type Labels = HashMap<String, String>;

/// Meant to stay up: a restart policy, or a compose service (but not a `compose run` one-off).
pub fn is_workload(restart: Option<&RestartPolicyNameEnum>, labels: &Labels) -> bool {
    use RestartPolicyNameEnum::*;
    let policy = matches!(restart, Some(ALWAYS | UNLESS_STOPPED | ON_FAILURE));
    let compose = labels.contains_key("com.docker.compose.service")
        && labels.get("com.docker.compose.oneoff").is_none_or(|v| v != "True");
    policy || compose
}

/// Compose: `(project, service)`, replica n > 1 as `service#n`. Otherwise `("-", name)`.
pub fn workload_key(host: &str, name: &str, labels: &Labels) -> WorkloadKey {
    let label = |k: &str| labels.get(&format!("{COMPOSE}{k}"));
    let (project, service) = match (label("project"), label("service")) {
        (Some(p), Some(s)) => match label("container-number").filter(|n| *n != "1") {
            Some(n) => (p.clone(), format!("{s}#{n}")),
            None => (p.clone(), s.clone()),
        },
        _ => ("-".to_string(), name.trim_start_matches('/').to_string()),
    };
    WorkloadKey { host: host.to_string(), project, service }
}

/// By the image's repository name, ignoring registry, tag and digest.
pub fn classify(image: &str) -> (WorkloadKind, Option<DatastoreKind>) {
    let repo = image.split('@').next().unwrap_or(image);
    let name = repo.rsplit('/').next().unwrap_or(repo);
    let name = name.split(':').next().unwrap_or(name);
    let datastore = match name {
        "postgres" | "postgis" | "pgvector" | "postgresql" | "timescaledb" | "pgvecto-rs" => {
            Some(DatastoreKind::Postgres)
        }
        "redis" | "valkey" => Some(DatastoreKind::Redis),
        "mysql" | "mariadb" => Some(DatastoreKind::Mysql),
        _ => None,
    };
    let kind = match (datastore, name) {
        (Some(_), _) => WorkloadKind::Datastore,
        (None, "nginx" | "caddy" | "traefik" | "haproxy" | "envoy") => WorkloadKind::Proxy,
        _ => WorkloadKind::App,
    };
    (kind, datastore)
}

pub fn facts_of(i: &ContainerInspectResponse) -> WorkloadFacts {
    let config = i.config.as_ref();
    let host = i.host_config.as_ref();
    let image = config.and_then(|c| c.image.clone()).unwrap_or_default();
    let (kind, datastore) = classify(&image);
    let log = host.and_then(|h| h.log_config.as_ref());
    let healthcheck = config
        .and_then(|c| c.healthcheck.as_ref()?.test.as_ref())
        .is_some_and(|t| !t.is_empty() && t[0] != "NONE");
    WorkloadFacts {
        kind,
        datastore,
        image,
        image_digest: i.image.clone(),
        created: i.created.as_deref().and_then(timestamp),
        restart_policy: host
            .and_then(|h| h.restart_policy.as_ref()?.name.as_ref())
            .map(ToString::to_string)
            .filter(|p| !p.is_empty() && p != "no"),
        ports: ports(i),
        memory_limit_bytes: host.and_then(|h| h.memory).filter(|m| *m > 0).map(|m| m as u64),
        labels: labels(i)
            .iter()
            .filter(|(k, _)| k.starts_with(COMPOSE))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
        healthcheck,
        log_driver: log.and_then(|l| l.typ.clone()),
        log_max_size: log.and_then(|l| l.config.as_ref()?.get("max-size").cloned()),
    }
}

pub fn labels(i: &ContainerInspectResponse) -> &Labels {
    static EMPTY: std::sync::LazyLock<Labels> = std::sync::LazyLock::new(Labels::new);
    i.config.as_ref().and_then(|c| c.labels.as_ref()).unwrap_or(&EMPTY)
}

/// `"0.0.0.0:4001->4001/tcp"`, sorted.
fn ports(i: &ContainerInspectResponse) -> Vec<String> {
    let bindings = i.host_config.as_ref().and_then(|h| h.port_bindings.as_ref());
    let mut ports: Vec<String> = bindings
        .into_iter()
        .flatten()
        .flat_map(|(container, hosts)| {
            hosts.iter().flatten().map(move |b| {
                let ip = b.host_ip.as_deref().filter(|ip| !ip.is_empty()).unwrap_or("0.0.0.0");
                format!("{ip}:{}->{container}", b.host_port.as_deref().unwrap_or(""))
            })
        })
        .collect();
    ports.sort();
    ports
}

pub fn state_of(
    i: &ContainerInspectResponse,
    restarts: Vec<Timestamp>,
    memory: Option<u64>,
) -> WorkloadState {
    use ContainerStateStatusEnum as S;
    let state = i.state.as_ref();
    let run = match state.and_then(|s| s.status) {
        Some(S::RUNNING) => RunState::Running,
        Some(S::RESTARTING) => RunState::Restarting,
        Some(S::PAUSED) => RunState::Paused,
        Some(S::EXITED) => RunState::Exited,
        Some(S::DEAD | S::REMOVING) => RunState::Dead,
        Some(S::CREATED) => RunState::Created,
        _ => RunState::Unknown,
    };
    let health = state.and_then(|s| s.health.as_ref()?.status).and_then(|h| match h {
        HealthStatusEnum::HEALTHY => Some(Health::Healthy),
        HealthStatusEnum::UNHEALTHY => Some(Health::Unhealthy),
        HealthStatusEnum::STARTING => Some(Health::Starting),
        _ => None,
    });
    WorkloadState {
        run,
        exit_code: state.and_then(|s| s.exit_code).filter(|_| run == RunState::Exited),
        health,
        restarts,
        memory_used_bytes: memory,
        missing_ports: Vec::new(),
        datastore: None,
    }
}

/// One container per workload key — running first, then the newest, so a redeploy's old
/// container never hides the new one. Containers that are not workloads are only counted.
pub fn winners<'a>(
    host: &str,
    inspects: &'a [ContainerInspectResponse],
) -> (Vec<(WorkloadKey, &'a ContainerInspectResponse)>, TransientCounts) {
    let running =
        |i: &ContainerInspectResponse| i.state.as_ref().and_then(|s| s.running) == Some(true);
    let rank = |i: &ContainerInspectResponse| {
        (running(i), i.created.as_deref().and_then(timestamp), i.id.clone())
    };
    let (by_key, transient) = inspects.iter().fold(
        (BTreeMap::new(), TransientCounts::default()),
        |(mut by_key, mut transient), i| {
            let policy =
                i.host_config.as_ref().and_then(|h| h.restart_policy.as_ref()?.name.as_ref());
            if !is_workload(policy, labels(i)) {
                *if running(i) { &mut transient.running } else { &mut transient.exited } += 1;
                return (by_key, transient);
            }
            let key = workload_key(host, i.name.as_deref().unwrap_or_default(), labels(i));
            if by_key.get(&key).is_none_or(|old: &&ContainerInspectResponse| rank(i) > rank(old)) {
                by_key.insert(key, i);
            }
            (by_key, transient)
        },
    );
    (by_key.into_iter().collect(), transient)
}

/// Docker reports unset dates as `0001-01-01T00:00:00Z`.
pub fn timestamp(s: &str) -> Option<Timestamp> {
    s.parse::<Timestamp>().ok().filter(|t| t.as_second() > 0)
}

/// `docker stats` memory: usage minus inactive page cache.
pub fn working_set(memory_current: u64, memory_stat: &str) -> u64 {
    let inactive = memory_stat
        .lines()
        .find_map(|l| l.strip_prefix("inactive_file "))
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(0);
    memory_current.saturating_sub(inactive)
}

#[cfg(test)]
#[path = "containers_tests.rs"]
mod tests;
