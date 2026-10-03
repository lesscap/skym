//! Background work: heartbeat evaluation, endpoint probes and daily maintenance.

use crate::config::ServerConfig;
use crate::evaluate::{heartbeat_once, probes_once};
use crate::lifecycle;
use crate::probe::{self, Prober};
use crate::store::{Store, history, hosts, incidents};
use jiff::{SignedDuration, Timestamp};
use skym_core::subject::Subject;
use std::sync::Arc;
use std::time::Duration;

/// A quarter of the report interval, between 1 and 15 seconds.
pub fn heartbeat_period(report_interval: SignedDuration) -> Duration {
    let quarter = report_interval.as_secs() / 4;
    Duration::from_secs(quarter.clamp(1, 15) as u64)
}

pub fn spawn(store: Store, cfg: Arc<ServerConfig>, started: Timestamp) {
    spawn_probes(store.clone(), cfg.clone());
    let beat = store.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(heartbeat_period(cfg.report_interval));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let configured: Vec<String> = cfg.hosts.iter().map(|h| h.id.clone()).collect();
        let interval = cfg.report_interval;
        loop {
            tick.tick().await;
            let configured = configured.clone();
            let run = beat
                .call(move |c| heartbeat_once(c, &configured, started, interval, Timestamp::now()));
            if let Err(e) = run.await {
                tracing::error!("heartbeat: {e:#}");
            }
        }
    });
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(24 * 3600));
        loop {
            tick.tick().await;
            if let Err(e) = store.call(|c| maintain(c, Timestamp::now())).await {
                tracing::error!("maintenance: {e:#}");
            }
        }
    });
}

/// Probes every endpoint once per report interval. Runs with no endpoints too, so the
/// incidents of endpoints removed from the configuration are retired.
fn spawn_probes(store: Store, cfg: Arc<ServerConfig>) {
    let prober = match Prober::new(probe::TIMEOUT) {
        Ok(p) => Arc::new(p),
        Err(e) => return tracing::error!("probes disabled, no HTTP client: {e:#}"),
    };
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(cfg.report_interval.unsigned_abs());
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            let probed = prober.all(&cfg.endpoints).await;
            let cfg = cfg.clone();
            let run =
                store.call(move |c| probes_once(c, &cfg.endpoints, &probed, Timestamp::now()));
            if let Err(e) = run.await {
                tracing::error!("probes: {e:#}");
            }
        }
    });
}

/// Archives workloads unseen for a week and applies retention.
pub fn maintain(c: &mut rusqlite::Connection, now: Timestamp) -> anyhow::Result<()> {
    let days = |d: i64| now - SignedDuration::from_hours(24 * d);
    let tx = c.transaction()?;
    hosts::archive_stale(&tx, days(7), now)?;
    for key in hosts::archived(&tx)? {
        for active in incidents::active_for(&tx, &Subject::Workload(key))? {
            incidents::apply(&tx, &lifecycle::retire(&active, now), now)?;
        }
    }
    hosts::prune_disk_samples(&tx, days(7))?;
    hosts::prune_archived(&tx, days(90))?;
    history::prune(&tx, days(90), days(30))?;
    incidents::prune_resolved(&tx, days(90))?;
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heartbeat_checks_a_quarter_interval_within_bounds() {
        let p = |secs| heartbeat_period(SignedDuration::from_secs(secs)).as_secs();
        assert_eq!((p(60), p(2), p(600)), (15, 1, 15));
        assert_eq!(p(20), 5);
    }
}
