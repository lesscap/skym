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

impl WorkloadKey {
    /// Exactly the keys whose subject string parses back: the service may contain `/`,
    /// the project may not.
    pub fn is_valid(&self) -> bool {
        valid_host(&self.host)
            && !self.project.is_empty()
            && !self.project.contains('/')
            && !self.service.is_empty()
    }
}

/// An application: a compose project, or a lone container (project `-`) or systemd unit
/// (`_systemd`), which have no project to group them. Written `<host>/<project>` or
/// `<host>/<project>/<service>`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AppKey {
    pub host: HostId,
    pub project: String,
    pub service: Option<String>,
}

/// Projects that name no application: each of their workloads is one.
fn lone(project: &str) -> bool {
    project == "-" || project == "_systemd"
}

impl AppKey {
    /// A systemd unit: declared in the agent's configuration, so its absence is `WORKLOAD_DOWN`.
    pub fn is_unit(&self) -> bool {
        self.project == "_systemd"
    }

    /// The application a workload belongs to.
    pub fn of(w: &WorkloadKey) -> AppKey {
        AppKey {
            host: w.host.clone(),
            project: w.project.clone(),
            service: lone(&w.project).then(|| w.service.clone()),
        }
    }

    pub fn contains(&self, w: &WorkloadKey) -> bool {
        *self == AppKey::of(w)
    }
}

impl fmt::Display for AppKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.host, self.project)?;
        self.service.as_ref().map_or(Ok(()), |s| write!(f, "/{s}"))
    }
}

impl FromStr for AppKey {
    type Err = SubjectError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || SubjectError(s.to_string());
        let (host, rest) = s.split_once('/').ok_or_else(err)?;
        let (project, service) = match rest.split_once('/') {
            Some((p, svc)) => (p, Some(svc.to_string())),
            None => (rest, None),
        };
        let key = AppKey { host: host.into(), project: project.into(), service };
        let valid = valid_host(host)
            && !project.is_empty()
            && key.service.as_ref().is_some_and(|s| !s.is_empty()) == lone(project);
        valid.then_some(key).ok_or_else(err)
    }
}

impl Serialize for AppKey {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for AppKey {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?.parse().map_err(de::Error::custom)
    }
}

impl JsonSchema for AppKey {
    fn schema_name() -> Cow<'static, str> {
        "AppKey".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "description": "<host>/<project>, or <host>/<project>/<service> for a lone container (project -) or systemd unit (_systemd)"
        })
    }
}

/// What an incident or event is about. Serialized as one canonical string:
/// `host:<h>`, `workload:<h>/<project>/<service>`, `mount:<h>:<path>`, `endpoint:<url>`,
/// `app:<app key>`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Subject {
    Host(HostId),
    Workload(WorkloadKey),
    Mount {
        host: HostId,
        path: String,
    },
    Endpoint(String),
    App(AppKey),
    /// A kind a newer server added, kept verbatim. Only JSON reads produce it: parsing a
    /// string (configuration, storage) rejects what it does not know.
    Unknown(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubjectError(pub String);

impl fmt::Display for SubjectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid subject: {}", self.0)
    }
}

impl std::error::Error for SubjectError {}

impl Subject {
    /// The host the subject belongs to; endpoints belong to none.
    pub fn host(&self) -> Option<&HostId> {
        match self {
            Subject::Host(h) | Subject::Mount { host: h, .. } => Some(h),
            Subject::Workload(k) => Some(&k.host),
            Subject::App(a) => Some(&a.host),
            Subject::Endpoint(_) | Subject::Unknown(_) => None,
        }
    }
}

impl fmt::Display for Subject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Subject::Host(h) => write!(f, "host:{h}"),
            Subject::Workload(k) => write!(f, "workload:{}/{}/{}", k.host, k.project, k.service),
            Subject::Mount { host, path } => write!(f, "mount:{host}:{path}"),
            Subject::Endpoint(url) => write!(f, "endpoint:{url}"),
            Subject::App(a) => write!(f, "app:{a}"),
            Subject::Unknown(s) => f.write_str(s),
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
                (valid_host(host) && path.starts_with('/'))
                    .then(|| Subject::Mount { host: host.to_string(), path: path.to_string() })
            }),
            "endpoint" => (!rest.is_empty()).then(|| Subject::Endpoint(rest.to_string())),
            "app" => rest.parse().ok().map(Subject::App),
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
    let key = WorkloadKey {
        host: host.to_string(),
        project: project.to_string(),
        service: service.to_string(),
    };
    key.is_valid().then_some(key)
}

impl Serialize for Subject {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

const KINDS: [&str; 5] = ["host", "workload", "mount", "endpoint", "app"];

impl<'de> Deserialize<'de> for Subject {
    /// A malformed subject of a known kind is an error; one of an unknown kind is kept.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        match s.split_once(':') {
            Some((kind, _)) if !KINDS.contains(&kind) => Ok(Subject::Unknown(s)),
            _ => s.parse().map_err(de::Error::custom),
        }
    }
}

impl JsonSchema for Subject {
    fn schema_name() -> Cow<'static, str> {
        "Subject".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "description": "host:<h> | workload:<h>/<project>/<service> | mount:<h>:<path> | endpoint:<url> | app:<h>/<project>[/<service>]; consumers keep other kinds a newer server adds"
        })
    }
}
