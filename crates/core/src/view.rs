//! JSON shapes shared by the query API and `skym status --json`.

use crate::model::{HostFacts, HostState, RunState, WorkloadKind};
use crate::rules::{IncidentCode, Severity};
use crate::subject::{CustomerId, HostId, Subject, WorkloadKey};
use crate::time::Timestamp;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// `Unknown` means no data, e.g. a host that never reported.
#[derive(
    Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord,
)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Ok,
    Warn,
    Critical,
    Unknown,
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
}

/// Highest severity among unmuted incidents; `Ok` when there are none.
pub fn rollup(incidents: &[IncidentView]) -> Status {
    incidents
        .iter()
        .filter(|i| !i.muted)
        .map(|i| i.severity)
        .max()
        .map_or(Status::Ok, Status::from)
}
