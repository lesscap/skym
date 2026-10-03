//! `skym agent`: collect and report every interval, until SIGTERM or SIGINT.

use crate::collect::{Collected, Window, collect, host};
use crate::config::Config;
use crate::cursors::{self, Cursors};
use crate::deliver::{self, Answer, Client, Verdict, verdict};
use crate::exceptions::Detail;
use crate::facts::FactsPolicy;
use crate::memory::Memory;
use crate::outbox::{self, Outbox};
use crate::report;
use anyhow::Context;
use jiff::Timestamp;
use std::path::PathBuf;
use tokio::signal::unix::{SignalKind, signal};
use tokio::time::MissedTickBehavior;

struct Agent<'a> {
    cfg: &'a Config,
    host: String,
    client: Client,
    outbox: Outbox,
    cursors: Cursors,
    cursors_path: PathBuf,
    memory: Memory,
    facts: FactsPolicy,
    /// When the last accounted-for pass started: where reading starts for a workload
    /// without a cursor (one that never logged).
    last_read: Option<Timestamp>,
}

pub async fn run(cfg: &Config) -> anyhow::Result<u8> {
    let server = cfg.server.as_deref().context("`server` is not set in the configuration")?;
    let client = Client::new(server, deliver::read_token(&cfg.token_file)?)?;
    let cursors_path = cfg.state_dir.join("cursors.json");
    let mut agent = Agent {
        cfg,
        host: host::hostname(),
        client,
        outbox: Outbox::open(&cfg.state_dir)?,
        cursors: Cursors::load(&cursors_path).unwrap_or_else(|e| {
            eprintln!("warning: {e:#}; logs are read from the last interval");
            Cursors::default()
        }),
        cursors_path,
        memory: Memory::default(),
        facts: FactsPolicy::default(),
        last_read: None,
    };
    let mut ticks = tokio::time::interval(cfg.interval.unsigned_abs());
    ticks.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let (mut term, mut int) = (signal(SignalKind::terminate())?, signal(SignalKind::interrupt())?);
    eprintln!(
        "skym {} reporting to {server} every {}",
        env!("CARGO_PKG_VERSION"),
        skym_core::time::format_duration(cfg.interval)
    );
    loop {
        tokio::select! {
            _ = ticks.tick() => agent.pass().await,
            _ = term.recv() => break,
            _ = int.recv() => break,
        }
    }
    eprintln!("skym stopping");
    Ok(0)
}

impl Agent<'_> {
    /// Collect, store, then send. The pass's log lines count as read once its report is
    /// stored (or, when storing fails, delivered): a crash in between reads them again
    /// rather than losing them.
    async fn pass(&mut self) {
        let now = Timestamp::now();
        let starts = self.cursors.starts(now);
        let window = Window {
            now,
            events_since: self.memory.events_since(now),
            logs_after: cursors::default_start(self.last_read, now - self.cfg.interval, now),
            cursors: &starts,
            detail: Detail::Report,
        };
        let mut c = collect(self.cfg, &self.host, &window).await;
        c.errors.iter().for_each(|e| eprintln!("error: {e}"));
        self.memory.remember(&mut c, now);
        let mut report = report::build(&c, &self.host, now);
        self.facts.strip(&mut report);
        let accounted = match self.outbox.push(&report) {
            Ok(()) => {
                self.read_through(&c, now);
                self.flush().await;
                true
            }
            Err(e) => {
                eprintln!("error: {e:#}; sending this report without storing it");
                self.flush().await;
                self.send_now(&report).await
            }
        };
        if !accounted {
            self.facts = FactsPolicy::default(); // the server may lack these facts: resend all
        }
    }

    fn read_through(&mut self, c: &Collected, now: Timestamp) {
        self.cursors.advance(&c.log_ends);
        if c.workloads_complete {
            self.cursors.keep_only(c.workloads.iter().map(|w| &w.key));
        }
        if let Err(e) = self.cursors.save(&self.cursors_path) {
            eprintln!("error: {e:#}");
        }
        self.last_read = Some(now);
    }

    /// Sends stored reports oldest first; stops at the first that has to wait.
    async fn flush(&self) {
        match self.outbox.prune(Timestamp::now()) {
            Ok(0) => {}
            Ok(n) => eprintln!("warning: dropped {n} reports that could not be delivered in time"),
            Err(e) => eprintln!("error: {e:#}"),
        }
        let pending = match self.outbox.pending() {
            Ok(pending) => pending,
            Err(e) => return eprintln!("error: cannot list stored reports: {e:#}"),
        };
        for (i, path) in pending.iter().enumerate() {
            match std::fs::read(path) {
                Err(e) => eprintln!("error: cannot read {}: {e}; dropping it", path.display()),
                Ok(body) => {
                    let answer = self.client.send(body).await;
                    if !self.settled(&answer, pending.len() - i) {
                        return;
                    }
                }
            }
            if let Err(e) = std::fs::remove_file(path) {
                eprintln!("error: cannot remove {}: {e}", path.display());
            }
        }
    }

    /// A report that could not be stored, sent at once. Whether the server has it or will
    /// never take it.
    async fn send_now(&self, report: &skym_core::report::Report) -> bool {
        match outbox::gzip(report) {
            Ok(body) => self.settled(&self.client.send(body).await, 1),
            Err(e) => {
                eprintln!("error: {e:#}");
                false
            }
        }
    }

    /// Whether the report is done with (delivered or rejected for good); logs why not.
    fn settled(&self, answer: &Answer, waiting: usize) -> bool {
        let status = answer.as_ref().ok().map(|(status, _)| *status);
        match (verdict(status), answer) {
            (Verdict::Delivered, _) => true,
            (Verdict::Rejected, Ok((status, body))) => {
                eprintln!("error: the server rejected a report ({status}): {body}");
                true
            }
            (_, Ok((401 | 403, _))) => {
                eprintln!(
                    "error: the server does not accept this host's token; {waiting} reports waiting"
                );
                false
            }
            (_, Ok((status, body))) => {
                eprintln!(
                    "warning: the server answered {status}: {body}; {waiting} reports waiting"
                );
                false
            }
            (_, Err(e)) => {
                eprintln!("warning: cannot reach the server: {e}; {waiting} reports waiting");
                false
            }
        }
    }
}
