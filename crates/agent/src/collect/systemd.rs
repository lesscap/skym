//! Services declared in `[[systemd]]`: unit state and listening ports.

use crate::config::SystemdUnit;
use skym_core::model::{RunState, WorkloadFacts, WorkloadKind, WorkloadState};
use skym_core::subject::WorkloadKey;
use std::collections::{BTreeMap, BTreeSet};

pub async fn collect(
    host: &str,
    units: &[SystemdUnit],
) -> (Vec<(WorkloadKey, WorkloadFacts, WorkloadState)>, Vec<String>) {
    let tables: Vec<String> = ["/proc/net/tcp", "/proc/net/tcp6"]
        .iter()
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .collect();
    let listening = listening(&tables);
    let mut workloads = Vec::new();
    let mut errors = Vec::new();
    if listening.is_none() && units.iter().any(|u| !u.ports.is_empty()) {
        errors.push("systemd: cannot read /proc/net/tcp; port checks skipped".to_string());
    }
    for unit in units {
        match show(&unit.unit).await.and_then(|s| unit_state(&parse_show(&s))) {
            Ok((run, memory)) => {
                workloads.push(workload(host, unit, run, memory, listening.as_ref()))
            }
            Err(e) => errors.push(format!("systemd {}: {e}", unit.unit)),
        }
    }
    (workloads, errors)
}

fn workload(
    host: &str,
    unit: &SystemdUnit,
    run: RunState,
    memory: Option<u64>,
    listening: Option<&BTreeSet<u16>>,
) -> (WorkloadKey, WorkloadFacts, WorkloadState) {
    let key =
        WorkloadKey { host: host.into(), project: "_systemd".into(), service: unit.unit.clone() };
    let facts = WorkloadFacts {
        kind: WorkloadKind::App,
        datastore: None,
        image: unit.unit.clone(),
        image_digest: None,
        created: None,
        restart_policy: None,
        ports: Vec::new(),
        memory_limit_bytes: None,
        labels: BTreeMap::new(),
        healthcheck: false,
        log_driver: None,
        log_max_size: None,
    };
    let state = WorkloadState {
        run,
        exit_code: None,
        health: None,
        restarts: Vec::new(),
        memory_used_bytes: memory,
        missing_ports: listening.map_or_else(Vec::new, |l| {
            unit.ports.iter().copied().filter(|p| !l.contains(p)).collect()
        }),
        datastore: None,
    };
    (key, facts, state)
}

/// Bounded: a wedged systemd must not stall the pass. The child is killed on timeout.
async fn show(unit: &str) -> Result<String, String> {
    let props = "LoadState,ActiveState,SubState,MemoryCurrent";
    let run = tokio::process::Command::new("systemctl")
        .args(["show", unit, "-p", props])
        .kill_on_drop(true)
        .output();
    let out = tokio::time::timeout(std::time::Duration::from_secs(3), run)
        .await
        .map_err(|_| "systemctl did not answer within 3s".to_string())?
        .map_err(|e| e.to_string())?;
    String::from_utf8(out.stdout).map_err(|e| e.to_string())
}

pub fn parse_show(s: &str) -> BTreeMap<&str, &str> {
    s.lines().filter_map(|l| l.split_once('=')).collect()
}

pub fn unit_state(props: &BTreeMap<&str, &str>) -> Result<(RunState, Option<u64>), String> {
    if props.get("LoadState") == Some(&"not-found") {
        return Err("unit not found".into());
    }
    let run = match (props.get("ActiveState").copied(), props.get("SubState").copied()) {
        (Some("active" | "reloading"), _) => RunState::Running,
        (Some("activating" | "deactivating"), _) => RunState::Restarting,
        (Some("failed"), _) => RunState::Dead,
        (Some(_), _) => RunState::Inactive,
        (None, _) => return Err("no ActiveState".into()),
    };
    Ok((run, props.get("MemoryCurrent").and_then(|m| m.parse().ok())))
}

/// Listening ports across the socket tables that could be read; `None` if none could.
pub fn listening(tables: &[String]) -> Option<BTreeSet<u16>> {
    (!tables.is_empty()).then(|| tables.iter().flat_map(|t| listening_ports(t)).collect())
}

/// Ports in state `0A` (LISTEN) from `/proc/net/tcp` or `tcp6`.
pub fn listening_ports(proc_net_tcp: &str) -> BTreeSet<u16> {
    proc_net_tcp
        .lines()
        .skip(1)
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            let port = f.get(1)?.rsplit_once(':')?.1;
            (f.get(3)? == &"0A").then(|| u16::from_str_radix(port, 16).ok())?
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listening_ports_only_in_listen_state() {
        let tcp = "  sl  local_address rem_address   st\n\
                   0: 00000000:01BB 00000000:0000 0A 00000000\n\
                   1: 0100007F:1F90 0100007F:D3C2 01 00000000\n";
        let tcp6 = "  sl  local_address rem_address st\n\
                    0: 00000000000000000000000000000000:0050 00000000000000000000000000000000:0000 0A 0\n";
        let tables = [tcp.to_string(), tcp6.to_string()];
        assert_eq!(listening(&tables), Some(BTreeSet::from([80, 443])));
        assert_eq!(listening(&[]), None, "no readable table is not \"nothing listens\"");
    }

    #[test]
    fn missing_ports_are_declared_but_not_listening() {
        let unit = SystemdUnit { unit: "xray".into(), ports: vec![443, 8443] };
        let listening = BTreeSet::from([443]);
        let (key, _, state) = workload("x", &unit, RunState::Running, None, Some(&listening));
        assert_eq!((key.project.as_str(), key.service.as_str()), ("_systemd", "xray"));
        assert_eq!(state.missing_ports, [8443]);
        let (_, _, unknown) = workload("x", &unit, RunState::Running, None, None);
        assert!(unknown.missing_ports.is_empty(), "unreadable sockets are not missing ports");
    }

    #[test]
    fn unit_states() {
        let state = |s: &str| unit_state(&parse_show(s));
        let active =
            state("LoadState=loaded\nActiveState=active\nSubState=running\nMemoryCurrent=1024\n");
        assert_eq!(active, Ok((RunState::Running, Some(1024))));
        let restarting =
            state("ActiveState=activating\nSubState=auto-restart\nMemoryCurrent=[not set]\n");
        assert_eq!(restarting, Ok((RunState::Restarting, None)));
        assert_eq!(state("ActiveState=failed\n"), Ok((RunState::Dead, None)));
        assert_eq!(state("ActiveState=reloading\n"), Ok((RunState::Running, None)));
        assert_eq!(
            state("ActiveState=activating\nSubState=start\n"),
            Ok((RunState::Restarting, None))
        );
        assert_eq!(state("ActiveState=deactivating\n"), Ok((RunState::Restarting, None)));
        assert_eq!(state("ActiveState=inactive\n"), Ok((RunState::Inactive, None)));
        assert!(state("LoadState=not-found\nActiveState=inactive\n").is_err());
    }
}
