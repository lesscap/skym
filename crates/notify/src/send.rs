//! The HTTP calls: reading skym's open incidents, posting to Feishu, pinging healthchecks.io.
//! Errors never carry a URL: the webhook and the ping URL are secrets.

use crate::config::Feishu;
use crate::message::sign;
use anyhow::{Context, bail};
use jiff::Timestamp;
use serde_json::{Value, json};
use skym_core::view::IncidentList;
use std::time::Duration;

pub fn client() -> anyhow::Result<reqwest::Client> {
    install_tls();
    Ok(reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .connect_timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("skym-notify/", env!("CARGO_PKG_VERSION")))
        .build()?)
}

/// Every open incident, muted ones included: one muted after its announcement is still
/// open, not resolved.
pub async fn read_open(
    client: &reqwest::Client,
    server: &str,
    token: &str,
) -> anyhow::Result<IncidentList> {
    let server = server.trim_end_matches('/');
    let url = format!("{server}/api/incidents?include_muted=true&limit=1000");
    let response = client.get(url).bearer_auth(token).send().await.map_err(bare)?;
    let status = response.status();
    if !status.is_success() {
        bail!("skym answered {status}");
    }
    response.json().await.map_err(bare).context("reading skym's incidents")
}

/// Delivered only when Feishu answers `"code": 0`; anything else is a failure.
pub async fn post(
    client: &reqwest::Client,
    feishu: &Feishu,
    text: &str,
    now: Timestamp,
) -> anyhow::Result<()> {
    let mut body = json!({ "msg_type": "text", "content": { "text": text } });
    if let Some(secret) = &feishu.secret {
        let ts = now.as_second();
        body["timestamp"] = ts.to_string().into();
        body["sign"] = sign(secret.expose(), ts).into();
    }
    let response = client.post(feishu.webhook.expose()).json(&body).send().await.map_err(bare)?;
    let status = response.status();
    let answer: Value = response
        .json()
        .await
        .map_err(bare)
        .with_context(|| format!("reading Feishu's answer ({status})"))?;
    match answer["code"].as_i64() {
        Some(0) => Ok(()),
        _ => bail!("Feishu answered {status}: code {}, {}", answer["code"], answer["msg"]),
    }
}

pub async fn ping(client: &reqwest::Client, url: &str, fail: bool) -> anyhow::Result<()> {
    let url = if fail { format!("{}/fail", url.trim_end_matches('/')) } else { url.to_string() };
    let status = client.get(url).send().await.map_err(bare)?.status();
    if !status.is_success() {
        bail!("healthchecks.io answered {status}");
    }
    Ok(())
}

fn bare(e: reqwest::Error) -> anyhow::Error {
    anyhow::Error::new(e.without_url())
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
