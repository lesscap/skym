use crate::model::{
    ExceptionGroup, HostFacts, HostState, LocalEvent, WorkloadFacts, WorkloadState,
};
use crate::subject::{HostId, WorkloadKey};
use crate::time::Timestamp;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fmt;

/// The only message `skym agent` sends. Every report is also a heartbeat.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct Report {
    /// Filled by the agent with its hostname; the server replaces it, see [`rehost`].
    pub host: HostId,
    pub ts: Timestamp,
    pub host_facts_hash: String,
    pub host_facts: Option<HostFacts>,
    pub host_state: HostState,
    #[serde(default)]
    pub workloads: Vec<WorkloadReport>,
    #[serde(default)]
    pub local_events: Vec<LocalEvent>,
    #[serde(default)]
    pub exceptions: Vec<ExceptionGroup>,
    /// Sources that failed in this pass. Subjects missing from a report with errors
    /// are unknown, not recovered.
    #[serde(default)]
    pub errors: Vec<String>,
    /// The reporting `skym`'s version, in every report (host facts carry it only now and then).
    pub agent_version: Option<String>,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct WorkloadReport {
    pub key: WorkloadKey,
    pub facts_hash: String,
    pub facts: Option<WorkloadFacts>,
    pub state: WorkloadState,
}

/// 16 lowercase hex characters: the first 8 bytes of SHA-256 over the JSON form.
/// Struct field order and `BTreeMap` keep the JSON bytes deterministic.
pub fn facts_hash<T: Serialize>(facts: &T) -> String {
    let json = serde_json::to_vec(facts).expect("facts serialize to JSON");
    let digest = Sha256::digest(json);
    digest[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// Replaces every host reference with the identity the server derived from the token.
pub fn rehost(report: Report, host: &str) -> Report {
    let key = |k: WorkloadKey| WorkloadKey { host: host.to_string(), ..k };
    Report {
        host: host.to_string(),
        workloads: report
            .workloads
            .into_iter()
            .map(|w| WorkloadReport { key: key(w.key), ..w })
            .collect(),
        local_events: report
            .local_events
            .into_iter()
            .map(|e| match e {
                LocalEvent::OomKilled { ts, workload } => {
                    LocalEvent::OomKilled { ts, workload: workload.map(key) }
                }
                other => other,
            })
            .collect(),
        exceptions: report
            .exceptions
            .into_iter()
            .map(|g| ExceptionGroup { workload: key(g.workload), ..g })
            .collect(),
        ..report
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReportError {
    InvalidWorkloadKey(WorkloadKey),
    DuplicateWorkloadKey(WorkloadKey),
}

impl fmt::Display for ReportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReportError::InvalidWorkloadKey(k) => write!(f, "invalid workload key {k:?}"),
            ReportError::DuplicateWorkloadKey(k) => write!(f, "duplicate workload key {k:?}"),
        }
    }
}

impl std::error::Error for ReportError {}

/// What `judge` and storage rely on: every workload key is valid, and each workload
/// appears once. Call after [`rehost`]; the server rejects reports that fail.
pub fn validate(report: &Report) -> Result<(), ReportError> {
    let mut workload_keys = report.workloads.iter().map(|w| &w.key);
    let event_keys = report.local_events.iter().filter_map(|e| match e {
        LocalEvent::OomKilled { workload, .. } => workload.as_ref(),
        LocalEvent::Unknown => None,
    });
    let exception_keys = report.exceptions.iter().map(|g| &g.workload);
    let invalid =
        workload_keys.clone().chain(event_keys).chain(exception_keys).find(|k| !k.is_valid());
    let mut seen = BTreeSet::new();
    let duplicate = workload_keys.find(|k| !seen.insert(*k));
    match (invalid, duplicate) {
        (Some(k), _) => Err(ReportError::InvalidWorkloadKey(k.clone())),
        (None, Some(k)) => Err(ReportError::DuplicateWorkloadKey(k.clone())),
        (None, None) => Ok(()),
    }
}
