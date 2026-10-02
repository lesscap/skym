use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::{borrow::Cow, fmt, str::FromStr};

pub type HostId = String;
pub type CustomerId = String;

/// Stable identity of a workload across container recreation.
///
/// `project` is the compose project, `-` for a plain container, `_systemd` for a systemd unit.
#[derive(
    Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord,
)]
pub struct WorkloadKey {
    pub host: HostId,
    pub project: String,
    pub service: String,
}

/// What an incident or event is about. Serialized as one canonical string:
/// `host:<h>`, `workload:<h>/<project>/<service>`, `mount:<h>:<path>`, `endpoint:<url>`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Subject {
    Host(HostId),
    Workload(WorkloadKey),
    Mount { host: HostId, path: String },
    Endpoint(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubjectError(pub String);

impl fmt::Display for SubjectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid subject: {}", self.0)
    }
}

impl std::error::Error for SubjectError {}

impl fmt::Display for Subject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Subject::Host(h) => write!(f, "host:{h}"),
            Subject::Workload(k) => write!(f, "workload:{}/{}/{}", k.host, k.project, k.service),
            Subject::Mount { host, path } => write!(f, "mount:{host}:{path}"),
            Subject::Endpoint(url) => write!(f, "endpoint:{url}"),
        }
    }
}

impl FromStr for Subject {
    type Err = SubjectError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || SubjectError(s.to_string());
        let (prefix, rest) = s.split_once(':').ok_or_else(err)?;
        let parsed = match prefix {
            "host" => valid_host(rest).then(|| Subject::Host(rest.to_string())),
            "workload" => parse_workload(rest).map(Subject::Workload),
            "mount" => rest.split_once(':').and_then(|(host, path)| {
                (valid_host(host) && path.starts_with('/')).then(|| Subject::Mount {
                    host: host.to_string(),
                    path: path.to_string(),
                })
            }),
            "endpoint" => (!rest.is_empty()).then(|| Subject::Endpoint(rest.to_string())),
            _ => None,
        };
        parsed.ok_or_else(err)
    }
}

fn valid_host(h: &str) -> bool {
    !h.is_empty() && !h.contains(['/', ':'])
}

fn parse_workload(rest: &str) -> Option<WorkloadKey> {
    let mut parts = rest.splitn(3, '/');
    let (host, project, service) = (parts.next()?, parts.next()?, parts.next()?);
    (valid_host(host) && !project.is_empty() && !service.is_empty()).then(|| WorkloadKey {
        host: host.to_string(),
        project: project.to_string(),
        service: service.to_string(),
    })
}

impl Serialize for Subject {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Subject {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(de::Error::custom)
    }
}

impl JsonSchema for Subject {
    fn schema_name() -> Cow<'static, str> {
        "Subject".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "pattern": "^(host|workload|mount|endpoint):",
            "description": "host:<h> | workload:<h>/<project>/<service> | mount:<h>:<path> | endpoint:<url>"
        })
    }
}
