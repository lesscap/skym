use crate::subject::Subject;
use crate::time::SignedDuration;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Not extended with `Unknown`: codes never travel inside a report, and a typo in
/// configuration should fail loudly.
#[derive(
    Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord,
)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum IncidentCode {
    HeartbeatLost,
    WorkloadDown,
    WorkloadUnhealthy,
    CrashLoop,
    OomKilled,
    DiskFilling,
    LogUnbounded,
    DatastoreUnreachable,
    ReplicationLag,
    EndpointDown,
    CertExpiring,
    AppExceptions,
}

#[derive(
    Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord,
)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Warn,
    Critical,
}

/// What a rule found in one evaluation.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct Finding {
    pub subject: Subject,
    pub code: IncidentCode,
    pub severity: Severity,
    pub detail: String,
}

/// How findings turn into incidents over consecutive evaluations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rule {
    pub code: IncidentCode,
    pub open_after: u32,
    pub resolve_after: u32,
    pub decay_to_warn_after: Option<SignedDuration>,
}

/// A resolved incident that matches again within this window is reopened.
pub const REOPEN_WINDOW: SignedDuration = SignedDuration::from_mins(30);

pub const fn rule(code: IncidentCode) -> Rule {
    use IncidentCode::*;
    let (open_after, resolve_after, decay_to_warn_after) = match code {
        WorkloadDown | DatastoreUnreachable | EndpointDown => (2, 2, None),
        WorkloadUnhealthy => (2, 2, Some(SignedDuration::from_hours(24))),
        ReplicationLag => (1, 5, None),
        AppExceptions => (1, 30, None),
        HeartbeatLost | CrashLoop | OomKilled | DiskFilling | LogUnbounded | CertExpiring => {
            (1, 1, None)
        }
    };
    Rule { code, open_after, resolve_after, decay_to_warn_after }
}
