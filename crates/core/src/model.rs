//! Facts, state, events and exception groups. Every type tolerates unknown fields,
//! and every enum that crosses the wire maps unknown values to `Unknown`.

use crate::subject::{Subject, WorkloadKey};
use crate::time::Timestamp;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct HostFacts {
    pub hostname: String,
    pub os: String,
    pub kernel: String,
    pub arch: String,
    pub cpu_count: u32,
    pub memory_total_bytes: u64,
    pub boot_time: Timestamp,
    pub docker_version: Option<String>,
    pub agent_version: String,
    #[serde(default)]
    pub mounts: Vec<MountFacts>,
    pub public_ip: Option<String>,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct MountFacts {
    pub path: String,
    pub fs_type: String,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Default)]
pub struct HostState {
    pub load_1m: f64,
    pub load_5m: f64,
    pub load_15m: f64,
    pub memory_used_bytes: u64,
    #[serde(default)]
    pub mounts: Vec<MountState>,
    #[serde(default)]
    pub transient_containers: TransientCounts,
    /// Since the previous pass, as a share of all CPUs (0–100): user, nice, system, irq and
    /// softirq. `None` on the first pass after a start, after a reboot, or across a long gap.
    #[serde(default)]
    pub cpu_percent: Option<f32>,
    #[serde(default)]
    pub iowait_percent: Option<f32>,
    /// Time a virtual machine waited for its hypervisor.
    #[serde(default)]
    pub steal_percent: Option<f32>,
    /// Since the previous pass, at the physical interfaces: traffic is counted once, where it
    /// enters or leaves the host (not on bonds, bridges or container interfaces, nor traffic
    /// that stays on the host).
    #[serde(default)]
    pub net_rx_bytes_per_s: Option<u64>,
    #[serde(default)]
    pub net_tx_bytes_per_s: Option<u64>,
}

/// Sizes live in state, not facts, so every report can be judged on its own.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct MountState {
    pub path: String,
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub inodes_total: u64,
    pub inodes_used: u64,
}

/// Containers that are not workloads (no restart policy, not in compose).
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Default)]
pub struct TransientCounts {
    pub running: u32,
    pub exited: u32,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkloadKind {
    App,
    Datastore,
    Proxy,
    #[serde(other)]
    Unknown,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DatastoreKind {
    Postgres,
    Redis,
    Mysql,
    #[serde(other)]
    Unknown,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct WorkloadFacts {
    pub kind: WorkloadKind,
    pub datastore: Option<DatastoreKind>,
    pub image: String,
    pub image_digest: Option<String>,
    pub created: Option<Timestamp>,
    pub restart_policy: Option<String>,
    #[serde(default)]
    pub ports: Vec<String>,
    pub memory_limit_bytes: Option<u64>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub healthcheck: bool,
    pub log_driver: Option<String>,
    pub log_max_size: Option<String>,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Running,
    Restarting,
    Paused,
    Exited,
    Dead,
    Created,
    Inactive,
    #[serde(other)]
    Unknown,
}

impl fmt::Display for RunState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            RunState::Running => "running",
            RunState::Restarting => "restarting",
            RunState::Paused => "paused",
            RunState::Exited => "exited",
            RunState::Dead => "dead",
            RunState::Created => "created",
            RunState::Inactive => "inactive",
            RunState::Unknown => "unknown",
        })
    }
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Health {
    Healthy,
    Unhealthy,
    Starting,
    #[serde(other)]
    Unknown,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct WorkloadState {
    pub run: RunState,
    pub exit_code: Option<i64>,
    pub health: Option<Health>,
    /// Crash restarts within the last hour.
    #[serde(default)]
    pub restarts: Vec<Timestamp>,
    pub memory_used_bytes: Option<u64>,
    /// systemd units: declared ports that are not listening.
    #[serde(default)]
    pub missing_ports: Vec<u16>,
    pub datastore: Option<DatastoreProbe>,
    /// Since when the workload is in its run state: started, or exited.
    #[serde(default)]
    pub state_since: Option<Timestamp>,
    /// The last exit was an out-of-memory kill.
    #[serde(default)]
    pub oom_killed: bool,
    /// Consecutive failed healthchecks, and the last check's output (truncated, redacted).
    #[serde(default)]
    pub health_failing_streak: Option<u32>,
    #[serde(default)]
    pub health_output: Option<String>,
}

impl WorkloadState {
    /// When the workload stopped, if it is stopped: the real start of its being down. A
    /// running one (a systemd unit that stopped listening) has no such time.
    pub fn stopped_since(&self) -> Option<Timestamp> {
        match self.run {
            RunState::Running | RunState::Unknown => None,
            _ => self.state_since,
        }
    }
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct DatastoreProbe {
    pub reachable: bool,
    pub detail: String,
    pub replication_lag_s: Option<f64>,
}

/// Events only visible on the host.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LocalEvent {
    OomKilled {
        ts: Timestamp,
        workload: Option<WorkloadKey>,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventKind {
    Deployed {
        from: String,
        to: String,
    },
    ConfigChanged,
    Restarted,
    OomKilled,
    HostRebooted,
    KernelChanged,
    #[serde(other)]
    Unknown,
}

impl EventKind {
    /// The wire `type` tag.
    pub const fn tag(&self) -> &'static str {
        match self {
            EventKind::Deployed { .. } => "deployed",
            EventKind::ConfigChanged => "config_changed",
            EventKind::Restarted => "restarted",
            EventKind::OomKilled => "oom_killed",
            EventKind::HostRebooted => "host_rebooted",
            EventKind::KernelChanged => "kernel_changed",
            EventKind::Unknown => "unknown",
        }
    }
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct Event {
    pub ts: Timestamp,
    pub subject: Subject,
    pub kind: EventKind,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExceptionClass {
    Application,
    Business,
    #[serde(other)]
    Unknown,
}

impl ExceptionClass {
    /// The wire name.
    pub const fn as_str(self) -> &'static str {
        match self {
            ExceptionClass::Application => "application",
            ExceptionClass::Business => "business",
            ExceptionClass::Unknown => "unknown",
        }
    }
}

impl std::str::FromStr for ExceptionClass {
    type Err = String;

    /// The inverse of `as_str`, `unknown` included: a class added later is stored as unknown.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        [ExceptionClass::Application, ExceptionClass::Business, ExceptionClass::Unknown]
            .into_iter()
            .find(|c| c.as_str() == s)
            .ok_or_else(|| format!("unknown exception class {s}"))
    }
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct ExceptionGroup {
    pub workload: WorkloadKey,
    pub class: ExceptionClass,
    pub component: String,
    pub code: String,
    pub count: u32,
    pub final_count: u32,
    pub first_seen: Timestamp,
    pub last_seen: Timestamp,
    #[serde(default)]
    pub biz_keys: Vec<String>,
    pub sample: Option<ExceptionSample>,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct ExceptionSample {
    pub message: String,
    pub exception_type: Option<String>,
    pub stacktrace: Option<String>,
}
