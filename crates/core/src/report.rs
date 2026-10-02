use crate::model::{
    ExceptionGroup, HostFacts, HostState, LocalEvent, WorkloadFacts, WorkloadState,
};
use crate::subject::{HostId, WorkloadKey};
use crate::time::Timestamp;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

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
    let key = |k: WorkloadKey| WorkloadKey {
        host: host.to_string(),
        ..k
    };
    Report {
        host: host.to_string(),
        workloads: report
            .workloads
            .into_iter()
            .map(|w| WorkloadReport {
                key: key(w.key),
                ..w
            })
            .collect(),
        local_events: report
            .local_events
            .into_iter()
            .map(|e| match e {
                LocalEvent::OomKilled { ts, workload } => LocalEvent::OomKilled {
                    ts,
                    workload: workload.map(key),
                },
                other => other,
            })
            .collect(),
        exceptions: report
            .exceptions
            .into_iter()
            .map(|g| ExceptionGroup {
                workload: key(g.workload),
                ..g
            })
            .collect(),
        ..report
    }
}
