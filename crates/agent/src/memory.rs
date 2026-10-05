//! What `skym agent` carries from one pass to the next. Pure but for one log line per
//! workload found without a CPU counter. Lost on restart, which simply makes the next pass a
//! first pass.

use crate::collect::Collected;
use crate::collect::host::{Counters, MAX_SPAN, Sample, cores, cpu_rates, net_rates};
use jiff::{SignedDuration, Timestamp};
use skym_core::model::{LocalEvent, RunState};
use skym_core::subject::{Subject, WorkloadKey};
use std::collections::{BTreeMap, BTreeSet};

const HOUR: SignedDuration = SignedDuration::from_hours(1);
/// A `die` within 15 s of a `kill` is a requested stop, so each events query reaches 15 s
/// back for that context.
const CONTEXT: SignedDuration = SignedDuration::from_secs(15);

#[derive(Default)]
pub struct Memory {
    /// The whole second the previous events query ended at (Docker's `until`).
    until: Option<Timestamp>,
    restarts: BTreeMap<WorkloadKey, BTreeSet<Timestamp>>,
    oom_kill_count: Option<u64>,
    unit_restart_counts: BTreeMap<WorkloadKey, u32>,
    counters: Counters,
    /// Running workloads found without a CPU counter, said once each.
    uncounted: BTreeSet<WorkloadKey>,
}

impl Memory {
    /// A memory that has only seen these counters: for a one-off pass that still shows rates.
    pub fn starting_from(counters: Counters) -> Memory {
        Memory { counters, ..Memory::default() }
    }

    /// Where this pass's container events start: 15 s before where the last successful
    /// query ended, and never more than an hour back (the whole hour at first).
    pub fn events_since(&self, now: Timestamp) -> Timestamp {
        self.until.map_or(now - HOUR, |t| (t - CONTEXT).max(now - HOUR))
    }

    /// Completes a pass with what earlier passes saw. Only restarts and OOM kills after the
    /// previous query's end are new: earlier ones were judged then, with their context.
    pub fn remember(&mut self, c: &mut Collected, now: Timestamp) {
        let fresh: Vec<(WorkloadKey, Vec<Timestamp>)> = c
            .workloads
            .iter()
            .map(|w| {
                let new = w.state.restarts.iter().copied().filter(|t| self.is_new(t));
                (w.key.clone(), new.collect())
            })
            .collect();
        for (key, times) in fresh.into_iter().chain(self.unit_restarts(c, now)) {
            self.restarts.entry(key).or_default().extend(times);
        }
        let hour_ago = now - HOUR;
        // A workload missing from an incomplete listing is unknown, not gone.
        self.restarts.retain(|key, times| {
            times.retain(|t| *t >= hour_ago);
            !times.is_empty()
                && (!c.workloads_complete || c.workloads.iter().any(|w| w.key == *key))
        });
        for w in &mut c.workloads {
            w.state.restarts = self.restarts.get(&w.key).into_iter().flatten().copied().collect();
        }
        if self.host_oom(c) {
            c.local_events.push(LocalEvent::OomKilled { ts: now, workload: None });
        }
        self.oom_kill_count = c.oom_kill_count.or(self.oom_kill_count);
        self.unit_restart_counts = c.unit_restart_counts.clone();
        if c.events_read {
            self.until = Some(whole_second(now));
        }
        self.rates(c);
        for key in self.uncounted(c) {
            let workload = Subject::Workload(key);
            eprintln!("warning: {workload}: no cgroup CPU counter found; its CPU stays unknown");
        }
    }

    /// Running workloads without a CPU counter not reported before. One that leaves is
    /// forgotten, so the set holds only what runs now.
    fn uncounted(&mut self, c: &Collected) -> Vec<WorkloadKey> {
        self.uncounted.retain(|key| c.workloads.iter().any(|w| w.key == *key));
        let counted = |key: &WorkloadKey| c.targets.iter().any(|t| t.key == *key);
        let running = c.workloads.iter().filter(|w| w.state.run == RunState::Running);
        let new: Vec<WorkloadKey> = running
            .map(|w| &w.key)
            .filter(|key| !counted(key) && !self.uncounted.contains(*key))
            .cloned()
            .collect();
        self.uncounted.extend(new.iter().cloned());
        new
    }

    /// CPU and network rates since the previous readings, into the host's state and each
    /// workload's. A reading that failed keeps the previous one, so the next rate spans both
    /// intervals; a workload's is kept while it could still make one.
    pub fn rates(&mut self, c: &mut Collected) {
        let (prev, cur) = (&self.counters, &c.counters);
        for w in &mut c.workloads {
            let pair = prev.workloads.get(&w.key).zip(cur.workloads.get(&w.key));
            w.state.cpu_cores = pair.and_then(|(p, n)| cores(p, n));
        }
        if let Some((_, state)) = &mut c.host {
            let cpu = prev.cpu.as_ref().zip(cur.cpu.as_ref()).and_then(|(p, n)| cpu_rates(p, n));
            if let Some((busy, iowait, steal)) = cpu {
                (state.cpu_percent, state.iowait_percent, state.steal_percent) =
                    (Some(busy), Some(iowait), Some(steal));
            }
            let net = prev.net.as_ref().zip(cur.net.as_ref()).and_then(|(p, n)| net_rates(p, n));
            if let Some((rx, tx)) = net {
                (state.net_rx_bytes_per_s, state.net_tx_bytes_per_s) = (Some(rx), Some(tx));
            }
        }
        let recent = |s: &Sample| cur.at.is_none_or(|now| now.saturating_sub(s.at) <= MAX_SPAN);
        let mut workloads = cur.workloads.clone();
        for (key, s) in prev.workloads.iter().filter(|(_, s)| recent(s)) {
            workloads.entry(key.clone()).or_insert_with(|| s.clone());
        }
        self.counters = Counters {
            at: cur.at.or(prev.at),
            cpu: cur.cpu.or(prev.cpu),
            net: cur.net.clone().or_else(|| prev.net.clone()),
            workloads,
        };
    }

    fn is_new(&self, t: &Timestamp) -> bool {
        self.until.is_none_or(|until| *t > until)
    }

    /// More OOM kills on the host than new container OOM kills account for.
    fn host_oom(&self, c: &Collected) -> bool {
        let containers = c
            .local_events
            .iter()
            .filter(
                |e| matches!(e, LocalEvent::OomKilled { workload: Some(_), ts } if self.is_new(ts)),
            )
            .count() as u64;
        match (self.oom_kill_count, c.oom_kill_count) {
            (Some(before), Some(count)) => count.saturating_sub(before) > containers,
            _ => false,
        }
    }

    /// systemd counts restarts but does not time them: each new one is placed a whole second
    /// apart, back from `now`, so the server keeps them as distinct events. A unit seen for
    /// the first time only sets the baseline.
    fn unit_restarts(&self, c: &Collected, now: Timestamp) -> Vec<(WorkloadKey, Vec<Timestamp>)> {
        let last = whole_second(now);
        c.unit_restart_counts
            .iter()
            .filter_map(|(key, count)| {
                let new = count.saturating_sub(*self.unit_restart_counts.get(key)?);
                let times = (0..new).map(|k| last - SignedDuration::from_secs(k.into()));
                Some((key.clone(), times.collect()))
            })
            .collect()
    }
}

fn whole_second(t: Timestamp) -> Timestamp {
    Timestamp::from_second(t.as_second()).expect("in range")
}

#[cfg(test)]
#[path = "memory_tests.rs"]
mod tests;
