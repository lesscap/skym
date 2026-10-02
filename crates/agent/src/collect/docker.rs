//! Docker Engine API calls. Read-only: list, inspect, events, logs (in `logs.rs`), info.
//! Every call has a deadline: bollard's own timeout does not cover reading a response body.

use super::containers::working_set;
use anyhow::{Context, anyhow};
use bollard::errors::Error;
use bollard::models::{ContainerInspectResponse, EventMessage};
use bollard::query_parameters::{EventsOptions, ListContainersOptions};
use bollard::{API_DEFAULT_VERSION, Docker};
use futures_util::{StreamExt, TryStreamExt, stream};
use jiff::Timestamp;
use std::collections::HashMap;
use std::future::Future;
use std::path::Path;
use std::time::Duration;

async fn within<T, E>(
    secs: u64,
    what: &str,
    call: impl Future<Output = Result<T, E>>,
) -> anyhow::Result<T>
where
    E: std::error::Error + Send + Sync + 'static,
{
    match tokio::time::timeout(Duration::from_secs(secs), call).await {
        Ok(result) => result.with_context(|| what.to_string()),
        Err(_) => Err(anyhow!("{what}: Docker did not answer within {secs}s")),
    }
}

/// Negotiates down to the daemon's API version (Docker 24 speaks 1.43).
pub async fn connect(socket: &Path) -> anyhow::Result<Docker> {
    let path = socket.to_str().context("socket path is not UTF-8")?;
    let docker = Docker::connect_with_unix(path, 10, API_DEFAULT_VERSION)?;
    within(5, "version", docker.negotiate_version()).await
}

#[derive(Default)]
pub struct Daemon {
    pub version: Option<String>,
    pub root_dir: Option<String>,
}

pub async fn daemon(docker: &Docker) -> anyhow::Result<Daemon> {
    let info = within(5, "info", docker.info()).await?;
    Ok(Daemon { version: info.server_version, root_dir: info.docker_root_dir })
}

/// Every container except excluded names. Containers removed meanwhile are skipped;
/// containers that cannot be inspected are reported and left out.
pub async fn inspect_all(
    docker: &Docker,
    exclude: &[String],
) -> anyhow::Result<(Vec<ContainerInspectResponse>, Vec<String>)> {
    let options = ListContainersOptions { all: true, ..Default::default() };
    let listed = within(10, "list", docker.list_containers(Some(options))).await?;
    let ids: Vec<String> = listed
        .into_iter()
        .filter(|c| {
            let names = c.names.iter().flatten();
            !names.map(|n| n.trim_start_matches('/')).any(|n| exclude.iter().any(|e| e == n))
        })
        .filter_map(|c| c.id)
        .collect();
    let results: Vec<anyhow::Result<Option<ContainerInspectResponse>>> = stream::iter(ids)
        .map(|id| async move {
            let call = async {
                match docker.inspect_container(&id, None).await {
                    Err(Error::DockerResponseServerError { status_code: 404, .. }) => Ok(None),
                    other => other.map(Some),
                }
            };
            within(5, &format!("inspect {id:.12}"), call).await
        })
        .buffer_unordered(8)
        .collect()
        .await;
    Ok(results.into_iter().fold((Vec::new(), Vec::new()), |(mut ok, mut errors), r| {
        match r {
            Ok(i) => ok.extend(i),
            Err(e) => errors.push(format!("{e:#}")),
        }
        (ok, errors)
    }))
}

/// Container lifecycle events Docker still has in memory (it keeps only the last 256).
pub async fn events(
    docker: &Docker,
    since: Timestamp,
    until: Timestamp,
) -> anyhow::Result<Vec<EventMessage>> {
    let actions = ["die", "kill", "stop", "oom"].map(String::from).to_vec();
    let filters =
        HashMap::from([("type".into(), vec!["container".into()]), ("event".into(), actions)]);
    let options = EventsOptions {
        since: Some(since.as_second().to_string()),
        until: Some(until.as_second().to_string()),
        filters: Some(filters),
    };
    within(5, "events", docker.events(Some(options)).try_collect()).await
}

/// cgroup v2 working set, for the systemd and cgroupfs layouts.
pub fn memory(id: &str) -> Option<u64> {
    [
        format!("/sys/fs/cgroup/system.slice/docker-{id}.scope"),
        format!("/sys/fs/cgroup/docker/{id}"),
    ]
    .iter()
    .find_map(|dir| {
        let current = std::fs::read_to_string(format!("{dir}/memory.current")).ok()?;
        let stat = std::fs::read_to_string(format!("{dir}/memory.stat")).unwrap_or_default();
        Some(working_set(current.trim().parse().ok()?, &stat))
    })
}
