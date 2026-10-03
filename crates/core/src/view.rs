//! JSON shapes shared by the query API and `skym status --json`.

use crate::model::{
    Event, EventKind, ExceptionGroup, HostFacts, HostState, RunState, WorkloadFacts, WorkloadKind,
    WorkloadState,
};
use crate::rules::{IncidentCode, Severity};
use crate::subject::{AppKey, CustomerId, HostId, Subject, WorkloadKey};
use crate::time::Timestamp;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// `Unknown` means no data, e.g. a host that never reported (and any status a newer server
/// adds). Ordered by urgency: knowing nothing about a host outranks a warning, not a
/// critical incident.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Ok,
    Warn,
    Critical,
    #[serde(other)]
    Unknown,
}

impl Status {
    const fn urgency(self) -> u8 {
        match self {
            Status::Ok => 0,
            Status::Warn => 1,
            Status::Unknown => 2,
            Status::Critical => 3,
        }
    }
}

impl Ord for Status {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.urgency().cmp(&other.urgency())
    }
}

impl PartialOrd for Status {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl From<Severity> for Status {
    fn from(s: Severity) -> Self {
        match s {
            Severity::Info => Status::Ok,
            Severity::Warn => Status::Warn,
            Severity::Critical => Status::Critical,
            Severity::Unknown => Status::Unknown,
        }
    }
}

/// An open incident from the server, or a finding from a local `skym status`
/// (then the time fields are `None` and `links` is empty).
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct IncidentView {
    pub subject: Subject,
    pub code: IncidentCode,
    pub severity: Severity,
    pub detail: String,
    pub opened_at: Option<Timestamp>,
    /// When the problem really began, if known; it may predate `opened_at` (and skym).
    #[serde(default)]
    pub since: Option<Timestamp>,
    pub open_for: Option<String>,
    pub resolved_at: Option<Timestamp>,
    #[serde(default)]
    pub muted: bool,
    pub mute_reason: Option<String>,
    #[serde(default)]
    pub links: BTreeMap<String, String>,
    /// The application it belongs to; `None` for a host's own problems (heartbeat, disks…).
    #[serde(default)]
    pub app: Option<AppKey>,
    /// Since when skym watches its subject (the host, or the probed URL): a problem opened
    /// about then may be older.
    #[serde(default)]
    pub observed_since: Option<Timestamp>,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct WorkloadSummary {
    pub key: WorkloadKey,
    /// `None` until the server has the workload's facts.
    pub kind: Option<WorkloadKind>,
    pub status: Status,
    pub run: RunState,
    /// With `run`: an exit code 0 is a finished job, anything else a failure.
    #[serde(default)]
    pub exit_code: Option<i64>,
    /// Since when it is in `run`: the real start of a problem, which may predate skym.
    #[serde(default)]
    pub state_since: Option<Timestamp>,
    pub image: Option<String>,
    #[serde(default)]
    pub links: BTreeMap<String, String>,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct HostView {
    pub id: HostId,
    pub customer: Option<CustomerId>,
    pub status: Status,
    pub last_report_ago: Option<String>,
    /// When skym first heard from the host: incidents open about that long may be older.
    #[serde(default)]
    pub observed_since: Option<Timestamp>,
    pub facts: Option<HostFacts>,
    pub state: Option<HostState>,
    #[serde(default)]
    pub workloads: Vec<WorkloadSummary>,
    #[serde(default)]
    pub incidents: Vec<IncidentView>,
    /// Sources that failed in the last pass.
    #[serde(default)]
    pub errors: Vec<String>,
    /// The applications on the host.
    #[serde(default)]
    pub apps: Vec<AppSummary>,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct HostOverview {
    pub id: HostId,
    pub status: Status,
    pub last_report_ago: Option<String>,
    #[serde(default)]
    pub observed_since: Option<Timestamp>,
    /// Open `info` incidents among `incidents`: hygiene that does not count to `status`.
    #[serde(default)]
    pub info_count: u32,
    #[serde(default)]
    pub incidents: Vec<IncidentView>,
    #[serde(default)]
    pub links: BTreeMap<String, String>,
    #[serde(default)]
    pub load_1m: Option<f64>,
    #[serde(default)]
    pub memory_used_bytes: Option<u64>,
    #[serde(default)]
    pub memory_total_bytes: Option<u64>,
    #[serde(default)]
    pub disks: Vec<DiskUse>,
    /// Applications on the host, and how many of them have problems.
    #[serde(default)]
    pub apps: u32,
    #[serde(default)]
    pub apps_in_trouble: u32,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct DiskUse {
    pub path: String,
    pub used_percent: u8,
    /// `DISK_FILLING` is open for it.
    #[serde(default)]
    pub filling: bool,
}

/// A URL the server probes. `Unknown` until its first probe.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct EndpointOverview {
    pub url: String,
    pub status: Status,
    pub last_probe_ago: Option<String>,
    /// When skym first probed it: incidents open about that long may be older.
    #[serde(default)]
    pub observed_since: Option<Timestamp>,
    /// The last answer's HTTP status and how long it took; `None` when there was no answer.
    #[serde(default)]
    pub http_status: Option<u16>,
    #[serde(default)]
    pub latency_ms: Option<u64>,
    #[serde(default)]
    pub cert_expires_at: Option<Timestamp>,
    #[serde(default)]
    pub incidents: Vec<IncidentView>,
    /// The application this URL probes, when it is one of an app's probes.
    #[serde(default)]
    pub app: Option<AppKey>,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct CustomerOverview {
    pub id: CustomerId,
    pub name: String,
    pub status: Status,
    #[serde(default)]
    pub hosts: Vec<HostOverview>,
    #[serde(default)]
    pub endpoints: Vec<EndpointOverview>,
}

/// `GET /api/overview`: where is something wrong right now.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct Overview {
    pub ts: Timestamp,
    pub status: Status,
    /// Grouped by customer, for views older than `problems` and `hosts`; to be removed.
    #[serde(default)]
    pub customers: Vec<CustomerOverview>,
    #[serde(default)]
    pub muted_count: u32,
    /// Open, unmuted incidents of every host and application.
    #[serde(default)]
    pub problems: Vec<IncidentView>,
    /// Every host, most urgent first.
    #[serde(default)]
    pub hosts: Vec<HostOverview>,
}

/// `GET /api/hosts`.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct HostList {
    pub hosts: Vec<HostOverview>,
}

/// One application: discovered from its workloads, described by the configuration.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct AppSummary {
    pub key: AppKey,
    /// The configured name, else the project (or the lone service).
    pub name: String,
    /// Free text from the configuration; `prod`, `pre` and `test` are the usual ones.
    pub env: Option<String>,
    pub note: Option<String>,
    /// Listed in the configuration: skym raises `APP_MISSING` when it disappears.
    #[serde(default)]
    pub configured: bool,
    pub status: Status,
    #[serde(default)]
    pub services: u32,
    #[serde(default)]
    pub running: u32,
    #[serde(default)]
    pub last_deployed: Option<Timestamp>,
    /// Its probes.
    #[serde(default)]
    pub endpoints: Vec<EndpointOverview>,
    /// Open, unmuted incidents of its workloads, its probes and the app itself.
    #[serde(default)]
    pub incidents: Vec<IncidentView>,
    #[serde(default)]
    pub links: BTreeMap<String, String>,
}

/// `GET /api/apps`.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct AppList {
    pub apps: Vec<AppSummary>,
}

/// `GET /api/apps/{host}/{project}[/{service}]`.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct AppView {
    pub app: AppSummary,
    #[serde(default)]
    pub workloads: Vec<WorkloadSummary>,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct WorkloadView {
    pub key: WorkloadKey,
    pub status: Status,
    pub facts: Option<WorkloadFacts>,
    pub state: WorkloadState,
    #[serde(default)]
    pub incidents: Vec<IncidentView>,
    #[serde(default)]
    pub exceptions: Vec<ExceptionGroup>,
    #[serde(default)]
    pub events: Vec<Event>,
    #[serde(default)]
    pub links: BTreeMap<String, String>,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct IncidentList {
    pub incidents: Vec<IncidentView>,
    #[serde(default)]
    pub truncated: bool,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct ExceptionList {
    pub exceptions: Vec<ExceptionGroup>,
    #[serde(default)]
    pub truncated: bool,
}

/// Incident changes and events, by time.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct Timeline {
    pub entries: Vec<TimelineEntry>,
    #[serde(default)]
    pub truncated: bool,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct TimelineEntry {
    pub ts: Timestamp,
    pub subject: Subject,
    #[serde(flatten)]
    pub entry: TimelineKind,
}

/// `Event` names its field: `EventKind` carries its own `type` tag.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TimelineKind {
    IncidentOpened {
        code: IncidentCode,
        severity: Severity,
        detail: String,
    },
    IncidentReopened {
        code: IncidentCode,
        severity: Severity,
        detail: String,
    },
    IncidentResolved {
        code: IncidentCode,
    },
    SeverityChanged {
        code: IncidentCode,
        severity: Severity,
    },
    Event {
        event: EventKind,
    },
    #[serde(other)]
    Unknown,
}

/// Highest severity among unmuted incidents; `Ok` when there are none.
pub fn rollup<'a>(incidents: impl IntoIterator<Item = &'a IncidentView>) -> Status {
    incidents
        .into_iter()
        .filter(|i| !i.muted)
        .map(|i| Status::from(i.severity))
        .max()
        .unwrap_or(Status::Ok)
}

/// A workload's line in a host view, its status rolled up from its own incidents.
pub fn workload_summary(
    key: WorkloadKey,
    facts: Option<&WorkloadFacts>,
    state: &WorkloadState,
    incidents: &[IncidentView],
    links: BTreeMap<String, String>,
) -> WorkloadSummary {
    let subject = Subject::Workload(key.clone());
    WorkloadSummary {
        status: rollup(incidents.iter().filter(|i| i.subject == subject)),
        kind: facts.map(|f| f.kind),
        image: facts.map(|f| f.image.clone()),
        run: state.run,
        exit_code: state.exit_code,
        state_since: state.state_since,
        links,
        key,
    }
}
