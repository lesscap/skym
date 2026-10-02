//! JSON shapes shared by the query API and `skym status --json`.

use crate::model::{
    Event, EventKind, ExceptionGroup, HostFacts, HostState, RunState, WorkloadFacts, WorkloadKind,
    WorkloadState,
};
use crate::rules::{IncidentCode, Severity};
use crate::subject::{CustomerId, HostId, Subject, WorkloadKey};
use crate::time::Timestamp;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// `Unknown` means no data, e.g. a host that never reported. Ordered by urgency:
/// knowing nothing about a host outranks a warning, not a critical incident.
#[derive(
    Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord,
)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Ok,
    Warn,
    Unknown,
    Critical,
}

impl From<Severity> for Status {
    fn from(s: Severity) -> Self {
        match s {
            Severity::Warn => Status::Warn,
            Severity::Critical => Status::Critical,
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
    pub open_for: Option<String>,
    pub resolved_at: Option<Timestamp>,
    #[serde(default)]
    pub muted: bool,
    pub mute_reason: Option<String>,
    #[serde(default)]
    pub links: BTreeMap<String, String>,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct WorkloadSummary {
    pub key: WorkloadKey,
    /// `None` until the server has the workload's facts.
    pub kind: Option<WorkloadKind>,
    pub status: Status,
    pub run: RunState,
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
    pub facts: Option<HostFacts>,
    pub state: Option<HostState>,
    #[serde(default)]
    pub workloads: Vec<WorkloadSummary>,
    #[serde(default)]
    pub incidents: Vec<IncidentView>,
    /// Sources that failed in the last pass.
    #[serde(default)]
    pub errors: Vec<String>,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct HostOverview {
    pub id: HostId,
    pub status: Status,
    pub last_report_ago: Option<String>,
    #[serde(default)]
    pub incidents: Vec<IncidentView>,
    #[serde(default)]
    pub links: BTreeMap<String, String>,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct CustomerOverview {
    pub id: CustomerId,
    pub name: String,
    pub status: Status,
    #[serde(default)]
    pub hosts: Vec<HostOverview>,
}

/// `GET /api/overview`: where is something wrong right now.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct Overview {
    pub ts: Timestamp,
    pub status: Status,
    #[serde(default)]
    pub customers: Vec<CustomerOverview>,
    #[serde(default)]
    pub muted_count: u32,
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
        .map(|i| i.severity)
        .max()
        .map_or(Status::Ok, Status::from)
}

/// A workload's line in a host view, its status rolled up from its own incidents.
pub fn workload_summary(
    key: WorkloadKey,
    facts: Option<&WorkloadFacts>,
    run: RunState,
    incidents: &[IncidentView],
    links: BTreeMap<String, String>,
) -> WorkloadSummary {
    let subject = Subject::Workload(key.clone());
    WorkloadSummary {
        status: rollup(incidents.iter().filter(|i| i.subject == subject)),
        kind: facts.map(|f| f.kind),
        image: facts.map(|f| f.image.clone()),
        run,
        links,
        key,
    }
}
