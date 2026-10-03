//! The query API of `skym-server`, read into the core view types.

use serde::de::DeserializeOwned;
use skym_core::subject::{AppKey, HostId, WorkloadKey};
use skym_core::view::{
    AppList, AppView, ExceptionList, HostView, IncidentList, Overview, Timeline, WorkloadView,
};
use std::time::Duration;

/// What to read; also what an answer belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    Overview,
    /// Open incidents, muted ones included (the overview leaves those out).
    Muted,
    Host(HostId),
    Workload(WorkloadKey),
    Exceptions(HostId),
    Timeline {
        host: HostId,
        workload: Option<WorkloadKey>,
        since: &'static str,
    },
    Apps,
    App(AppKey),
}

#[derive(Debug)]
pub enum Payload {
    Overview(Overview),
    Muted(IncidentList),
    Host(HostView),
    Workload(WorkloadView),
    Exceptions(ExceptionList),
    Timeline(Timeline),
    Apps(AppList),
    App(AppView),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FetchError {
    /// The token is wrong: nothing will work until it is fixed.
    Unauthorized,
    /// No answer (network, TLS, timeout); the reason as text.
    Unreachable(String),
    /// An answer that is not the expected view.
    Bad(String),
}

impl Request {
    /// The API path, with every segment and value percent-encoded.
    pub fn path(&self) -> String {
        match self {
            Request::Overview => "/api/overview".into(),
            Request::Muted => "/api/incidents?include_muted=true&limit=1000".into(),
            Request::Host(h) => format!("/api/hosts/{}", encode(h)),
            Request::Workload(k) => format!(
                "/api/hosts/{}/workloads/{}/{}",
                encode(&k.host),
                encode(&k.project),
                encode(&k.service)
            ),
            Request::Exceptions(h) => format!("/api/exceptions?host={}&since=1h", encode(h)),
            Request::Timeline { host, workload, since } => {
                let workload = workload.as_ref().map_or(String::new(), |k| {
                    format!("&workload={}/{}", encode(&k.project), encode(&k.service))
                });
                format!("/api/timeline?host={}{workload}&since={since}&limit=500", encode(host))
            }
            Request::Apps => "/api/apps".into(),
            Request::App(a) => {
                let service =
                    a.service.as_ref().map_or(String::new(), |s| format!("/{}", encode(s)));
                format!("/api/apps/{}/{}{service}", encode(&a.host), encode(&a.project))
            }
        }
    }
}

fn encode(segment: &str) -> String {
    segment
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

pub struct Client {
    http: reqwest::Client,
    server: String,
    token: String,
}

impl Client {
    pub fn new(server: &str, token: String) -> anyhow::Result<Self> {
        install_tls();
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(5))
            .user_agent(concat!("skym-view/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Client { http, server: server.to_string(), token })
    }

    pub fn server(&self) -> &str {
        &self.server
    }

    pub async fn fetch(&self, request: &Request) -> Result<Payload, FetchError> {
        Ok(match request {
            Request::Overview => Payload::Overview(self.get(request).await?),
            Request::Muted => Payload::Muted(self.get(request).await?),
            Request::Host(_) => Payload::Host(self.get(request).await?),
            Request::Workload(_) => Payload::Workload(self.get(request).await?),
            Request::Exceptions(_) => Payload::Exceptions(self.get(request).await?),
            Request::Timeline { .. } => Payload::Timeline(self.get(request).await?),
            Request::Apps => Payload::Apps(self.get(request).await?),
            Request::App(_) => Payload::App(self.get(request).await?),
        })
    }

    async fn get<T: DeserializeOwned>(&self, request: &Request) -> Result<T, FetchError> {
        let url = format!("{}{}", self.server, request.path());
        let response = self
            .http
            .get(url)
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(|e| FetchError::Unreachable(chain(&e)))?;
        let status = response.status().as_u16();
        let body = response.text().await.map_err(|e| FetchError::Unreachable(chain(&e)))?;
        match status {
            401 | 403 => Err(FetchError::Unauthorized),
            200 => serde_json::from_str(&body).map_err(|e| FetchError::Bad(e.to_string())),
            _ => Err(FetchError::Bad(format!("{status}: {body}"))),
        }
    }
}

/// ring is the only TLS provider in the build; installed once, before the first client.
fn install_tls() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        if rustls::crypto::ring::default_provider().install_default().is_err() {
            eprintln!("warning: a TLS provider was already installed");
        }
    });
}

/// reqwest keeps the cause (refused, timed out, bad certificate) in the source chain.
fn chain(e: &dyn std::error::Error) -> String {
    std::iter::successors(Some(e), |e| e.source())
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(": ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_encode_names_and_carry_the_window() {
        let key = WorkloadKey { host: "x".into(), project: "-".into(), service: "api#2".into() };
        assert_eq!(Request::Workload(key.clone()).path(), "/api/hosts/x/workloads/-/api%232");
        let timeline = Request::Timeline { host: "x".into(), workload: Some(key), since: "7d" };
        assert_eq!(timeline.path(), "/api/timeline?host=x&workload=-/api%232&since=7d&limit=500");
        let host_only = Request::Timeline { host: "x".into(), workload: None, since: "6h" };
        assert_eq!(host_only.path(), "/api/timeline?host=x&since=6h&limit=500");
        assert_eq!(Request::Exceptions("x".into()).path(), "/api/exceptions?host=x&since=1h");
        assert_eq!(Request::App("y/nile".parse().unwrap()).path(), "/api/apps/y/nile");
        assert_eq!(Request::App("i/-/hb bs".parse().unwrap()).path(), "/api/apps/i/-/hb%20bs");
    }
}
