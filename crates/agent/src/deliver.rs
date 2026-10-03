//! Talking to `skym-server`: reports out, nothing in but an acknowledgement.

use anyhow::Context;
use std::path::Path;
use std::time::Duration;

/// What a delivery attempt means for the stored report.
#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The server has it: delete it.
    Delivered,
    /// The server will never take it: delete it and go on with the next.
    Rejected,
    /// Try again next pass, and stop here to keep the order.
    Stop,
}

/// `None` is a failure before any HTTP status (network, TLS, timeout).
pub fn verdict(status: Option<u16>) -> Verdict {
    match status {
        Some(200..=299) => Verdict::Delivered,
        Some(400 | 413 | 422) => Verdict::Rejected,
        _ => Verdict::Stop,
    }
}

pub struct Client {
    http: reqwest::Client,
    server: String,
    token: String,
}

/// The server's answer, or why there was none (network, TLS, timeout).
pub type Answer = Result<(u16, String), String>;

impl Client {
    pub fn new(server: &str, token: String) -> anyhow::Result<Self> {
        install_tls();
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(5))
            .user_agent(concat!("skym/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Client { http, server: server.trim_end_matches('/').to_string(), token })
    }

    pub fn server(&self) -> &str {
        &self.server
    }

    /// One gzip-compressed report.
    pub async fn send(&self, gzipped: Vec<u8>) -> Answer {
        let request = self
            .http
            .post(format!("{}/api/report", self.server))
            .bearer_auth(&self.token)
            .header("Content-Type", "application/json")
            .header("Content-Encoding", "gzip")
            .body(gzipped);
        answer(request).await
    }

    pub async fn healthz(&self) -> Answer {
        answer(self.http.get(format!("{}/healthz", self.server))).await
    }
}

async fn answer(request: reqwest::RequestBuilder) -> Answer {
    let response = request.send().await.map_err(|e| chain(&e))?;
    let status = response.status().as_u16();
    Ok((status, response.text().await.unwrap_or_else(|e| format!("(no body: {e})"))))
}

/// ring is the build's only TLS provider; installed once, before the first client.
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

/// The host token, without surrounding whitespace.
pub fn read_token(path: &Path) -> anyhow::Result<String> {
    let token = std::fs::read_to_string(path)
        .with_context(|| format!("cannot read the token file {}", path.display()))?;
    let token = token.trim();
    anyhow::ensure!(!token.is_empty(), "the token file {} is empty", path.display());
    Ok(token.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_each_answer_means_for_the_stored_report() {
        assert_eq!(verdict(Some(200)), Verdict::Delivered);
        assert_eq!(verdict(Some(204)), Verdict::Delivered);
        for rejected in [400, 413, 422] {
            assert_eq!(verdict(Some(rejected)), Verdict::Rejected, "{rejected}");
        }
        for later in
            [None, Some(301), Some(401), Some(403), Some(404), Some(429), Some(500), Some(503)]
        {
            assert_eq!(verdict(later), Verdict::Stop, "{later:?}");
        }
    }
}
