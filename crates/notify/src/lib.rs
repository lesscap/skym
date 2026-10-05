//! skym-notify: a reader of skym's API that tells a Feishu group when problems appear,
//! worsen and resolve, and pings healthchecks.io while it can read skym.

pub mod config;
pub mod decide;
pub mod message;
pub mod send;
pub mod state;

use config::Config;
use jiff::{SignedDuration, Timestamp};
use state::State;

/// How often skym is read.
pub const PASS: SignedDuration = SignedDuration::from_secs(15);
/// How often healthchecks.io is pinged.
pub const PING: SignedDuration = SignedDuration::from_secs(60);
/// How long Feishu may fail before healthchecks.io is told.
pub const FAILING: SignedDuration = SignedDuration::from_mins(10);

pub struct Notifier {
    pub config: Config,
    client: reqwest::Client,
    /// What was announced. Kept here and saved after each change: a failed save is
    /// reported and retried every pass, but never makes a pass announce again.
    state: State,
    unsaved: bool,
    read_ok_at: Option<Timestamp>,
    failing_since: Option<Timestamp>,
}

#[derive(Debug, PartialEq)]
pub enum Ping {
    Ok,
    Fail,
}

impl Notifier {
    pub fn new(config: Config) -> anyhow::Result<Self> {
        let state = state::load(&config.state);
        let client = send::client()?;
        Ok(Notifier {
            config,
            client,
            state,
            unsaved: false,
            read_ok_at: None,
            failing_since: None,
        })
    }

    /// Reads skym, and when anything changed, tells Feishu, then remembers it.
    pub async fn pass(&mut self, now: Timestamp) -> anyhow::Result<()> {
        let c = &self.config;
        let list = send::read_open(&self.client, &c.server, c.token.expose()).await?;
        self.read_ok_at = Some(now);
        let (notices, next) = decide::decide(&list.incidents, !list.truncated, &self.state, now);
        if !notices.is_empty() {
            let text = message::render(&notices);
            if let Err(e) = send::post(&self.client, &c.feishu, &text, now).await {
                self.failing_since.get_or_insert(now);
                return Err(e);
            }
        }
        // Delivered, or nothing left to deliver: news that went away meanwhile is no failure.
        self.failing_since = None;
        if next != self.state {
            self.state = next;
            self.unsaved = true;
        }
        if self.unsaved {
            state::save(&c.state, &self.state)?;
            self.unsaved = false;
        }
        Ok(())
    }

    /// Nothing while skym could not be read for two passes: healthchecks.io then alerts on
    /// the silence. `Fail` once Feishu has failed for [`FAILING`].
    pub fn health(&self, now: Timestamp) -> Option<Ping> {
        let read = self.read_ok_at.is_some_and(|t| now.duration_since(t) <= PASS * 2);
        let failing = self.failing_since.is_some_and(|t| now.duration_since(t) >= FAILING);
        read.then_some(if failing { Ping::Fail } else { Ping::Ok })
    }

    pub async fn ping(&self, now: Timestamp) -> anyhow::Result<()> {
        let (Some(hc), Some(ping)) = (&self.config.healthchecks, self.health(now)) else {
            return Ok(());
        };
        send::ping(&self.client, hc.ping_url.expose(), ping == Ping::Fail).await
    }
}
