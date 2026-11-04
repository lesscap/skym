//! Probing configured URLs from the server: one GET each, its status or why there was none,
//! when the certificate it was served with expires, and whether the body says what it should.

use crate::config::Endpoint;
use jiff::Timestamp;
use reqwest::header::HeaderValue;
use reqwest::redirect::{Attempt, Policy};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;
use x509_cert::Certificate;
use x509_cert::der::Decode;

/// What one probe saw.
#[derive(Debug, Clone, PartialEq)]
pub struct Probe {
    pub at: Timestamp,
    /// The HTTP status, or why there was none (connection, TLS, timeout).
    pub response: Result<u16, String>,
    pub latency_ms: u64,
    /// When the certificate of the answering connection expires (https only).
    pub cert_not_after: Option<Timestamp>,
    /// Why an expected answer's body fails the endpoint's `json_equals`. Judged in the pass
    /// that saw it and not stored: the incident's detail carries it.
    pub body_error: Option<String>,
}

/// Whether a status is the answer expected: one in `expect`, by default anything below 400.
pub fn expected(status: u16, expect: &[u16]) -> bool {
    if expect.is_empty() { status < 400 } else { expect.contains(&status) }
}

pub const TIMEOUT: Duration = Duration::from_secs(10);
const MAX_REDIRECTS: usize = 5;
const CONCURRENCY: usize = 8;
/// A health answer is small; a bigger body is not one, and is not read further.
const MAX_BODY: usize = 64 * 1024;

pub struct Prober {
    client: reqwest::Client,
    timeout: Duration,
}

impl Prober {
    pub fn new(timeout: Duration) -> reqwest::Result<Self> {
        install_tls();
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .tls_info(true)
            // Seen from this host: a proxy would answer for the endpoint and hide its certificate.
            .no_proxy()
            // A fresh connection each time: DNS, connect and the certificate are probed too.
            .pool_max_idle_per_host(0)
            .redirect(Policy::custom(redirect))
            .user_agent(concat!("skym-server/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Prober { client, timeout })
    }

    pub async fn probe(&self, e: &Endpoint) -> Probe {
        let at = Timestamp::now();
        let started = Instant::now();
        let request = e.headers.iter().fold(self.client.get(&e.url), |r, (name, value)| {
            let mut value = HeaderValue::from_str(value).expect("validated with the configuration");
            value.set_sensitive(true);
            r.header(name, value)
        });
        let sent = request.send().await;
        let latency_ms = started.elapsed().as_millis() as u64;
        match sent {
            Ok(r) => {
                let status = r.status().as_u16();
                let cert_not_after = r
                    .extensions()
                    .get::<reqwest::tls::TlsInfo>()
                    .and_then(|t| t.peer_certificate())
                    .and_then(not_after);
                let checked = !e.json_equals.is_empty() && expected(status, &e.expect);
                let body_error = match checked {
                    true => self.body_error(r, &e.json_equals).await,
                    false => None,
                };
                Probe { at, response: Ok(status), latency_ms, cert_not_after, body_error }
            }
            Err(e) => Probe {
                at,
                response: Err(self.reason(&e)),
                latency_ms,
                cert_not_after: None,
                body_error: None,
            },
        }
    }

    /// The body, read up to `MAX_BODY` within the client's timeout, against the checks. Says
    /// which check failed, never what the body holds.
    async fn body_error(
        &self,
        mut r: reqwest::Response,
        checks: &BTreeMap<String, Value>,
    ) -> Option<String> {
        let mut body = Vec::new();
        loop {
            match r.chunk().await {
                Ok(Some(c)) if body.len() + c.len() > MAX_BODY => {
                    return Some(format!("body larger than {} KiB", MAX_BODY / 1024));
                }
                Ok(Some(c)) => body.extend_from_slice(&c),
                Ok(None) => break,
                Err(e) => return Some(format!("reading the body: {}", self.reason(&e))),
            }
        }
        let Ok(json) = serde_json::from_slice::<Value>(&body) else {
            return Some("body is not JSON".into());
        };
        checks.iter().find_map(|(pointer, want)| match json.pointer(pointer) {
            None => Some(format!("{pointer} is missing")),
            Some(got) if got != want => Some(format!("{pointer} is not the expected value")),
            Some(_) => None,
        })
    }

    /// Every endpoint with its probe, a few at a time. A probe that fails to run is left out
    /// (and logged): its endpoint is not seen this pass.
    pub async fn all(self: &Arc<Self>, endpoints: &[Endpoint]) -> Vec<(Endpoint, Probe)> {
        let permits = Arc::new(Semaphore::new(CONCURRENCY));
        let tasks: Vec<_> = endpoints
            .iter()
            .map(|e| {
                let (prober, permits, e) = (self.clone(), permits.clone(), e.clone());
                tokio::spawn(async move {
                    let _permit = permits.acquire_owned().await.expect("never closed");
                    let probe = prober.probe(&e).await;
                    (e, probe)
                })
            })
            .collect();
        let mut probed = Vec::with_capacity(tasks.len());
        for task in tasks {
            match task.await {
                Ok(pair) => probed.push(pair),
                Err(e) => tracing::error!("a probe failed to run: {e}"),
            }
        }
        probed
    }

    /// The innermost cause reads best ("Connection refused", "invalid peer certificate: Expired").
    fn reason(&self, e: &reqwest::Error) -> String {
        if e.is_timeout() {
            return format!("timeout after {:?}", self.timeout);
        }
        let root = std::iter::successors(std::error::Error::source(e), |s| s.source())
            .last()
            .map_or_else(|| e.to_string(), ToString::to_string);
        if e.is_connect() { format!("connect: {root}") } else { root }
    }
}

/// Redirects are followed within the probed origin only (scheme, host and port), so the
/// configured headers (a probe token) go nowhere else; another origin's 3xx is the answer.
fn redirect(a: Attempt) -> reqwest::redirect::Action {
    let from = a.previous().last();
    if !from.is_some_and(|from| from.origin() == a.url().origin()) {
        a.stop()
    } else if a.previous().len() > MAX_REDIRECTS {
        a.error("too many redirects")
    } else {
        a.follow()
    }
}

/// The expiry of a DER certificate.
pub fn not_after(der: &[u8]) -> Option<Timestamp> {
    let cert = Certificate::from_der(der).ok()?;
    let expiry = cert.tbs_certificate().validity().not_after.to_unix_duration();
    Timestamp::from_second(i64::try_from(expiry.as_secs()).ok()?).ok()
}

fn install_tls() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        if rustls::crypto::ring::default_provider().install_default().is_err() {
            tracing::warn!("a TLS provider was already installed");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_after_reads_the_certificate_expiry() {
        let der = include_bytes!("../tests/fixtures/cert.der");
        assert_eq!(not_after(der), Some("2036-01-01T00:00:00Z".parse().unwrap()));
        assert_eq!(not_after(b"not a certificate"), None);
    }
}
