use crate::subject::Subject;
use crate::time::SignedDuration;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `Unknown` stands for a code a newer server added, for consumers of the API; the server
/// never produces it, and its configuration rejects it (a typo there must fail loudly).
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
    #[serde(other)]
    Unknown,
}

/// Ordered by weight. `Info` is hygiene: an incident, but no reason to call a host unwell.
/// `Unknown` is a severity a newer server added, ranked highest to stay on the safe side.
#[derive(
    Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord,
)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warn,
    Critical,
    #[serde(other)]
    Unknown,
}

impl IncidentCode {
    pub const ALL: [IncidentCode; 12] = [
        IncidentCode::HeartbeatLost,
        IncidentCode::WorkloadDown,
        IncidentCode::WorkloadUnhealthy,
        IncidentCode::CrashLoop,
        IncidentCode::OomKilled,
        IncidentCode::DiskFilling,
        IncidentCode::LogUnbounded,
        IncidentCode::DatastoreUnreachable,
        IncidentCode::ReplicationLag,
        IncidentCode::EndpointDown,
        IncidentCode::CertExpiring,
        IncidentCode::AppExceptions,
    ];

    /// The wire name, as serialized.
    pub const fn as_str(self) -> &'static str {
        match self {
            IncidentCode::HeartbeatLost => "HEARTBEAT_LOST",
            IncidentCode::WorkloadDown => "WORKLOAD_DOWN",
            IncidentCode::WorkloadUnhealthy => "WORKLOAD_UNHEALTHY",
            IncidentCode::CrashLoop => "CRASH_LOOP",
            IncidentCode::OomKilled => "OOM_KILLED",
            IncidentCode::DiskFilling => "DISK_FILLING",
            IncidentCode::LogUnbounded => "LOG_UNBOUNDED",
            IncidentCode::DatastoreUnreachable => "DATASTORE_UNREACHABLE",
            IncidentCode::ReplicationLag => "REPLICATION_LAG",
            IncidentCode::EndpointDown => "ENDPOINT_DOWN",
            IncidentCode::CertExpiring => "CERT_EXPIRING",
            IncidentCode::AppExceptions => "APP_EXCEPTIONS",
            IncidentCode::Unknown => "UNKNOWN",
        }
    }
}

impl std::str::FromStr for IncidentCode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL.into_iter().find(|c| c.as_str() == s).ok_or_else(|| format!("unknown code {s}"))
    }
}

impl Severity {
    pub const fn as_str(self) -> &'static str {
        match self {
            Severity::Unknown => "unknown",
            Severity::Info => "info",
            Severity::Warn => "warn",
            Severity::Critical => "critical",
        }
    }
}

impl std::str::FromStr for Severity {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "info" => Ok(Severity::Info),
            "warn" => Ok(Severity::Warn),
            "critical" => Ok(Severity::Critical),
            _ => Err(format!("unknown severity {s}")),
        }
    }
}

/// What a rule found in one evaluation.
#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    pub subject: Subject,
    pub code: IncidentCode,
    pub severity: Severity,
    pub detail: String,
}

/// How findings turn into incidents over consecutive evaluations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rule {
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
        HeartbeatLost | CrashLoop | OomKilled | DiskFilling | LogUnbounded | CertExpiring
        | Unknown => (1, 1, None),
    };
    Rule { open_after, resolve_after, decay_to_warn_after }
}
