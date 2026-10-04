//! One read of what the views draw on, so every response attributes and dates its
//! problems the same way.

use super::apps::History;
use super::views::{self, Context, incident_view, links};
use super::{ApiError, AppState, apps};
use crate::lifecycle::Incident;
use crate::store::hosts::{HostRow, WorkloadRow};
use crate::store::probes::ProbeRow;
use crate::store::{history, hosts, incidents, probes};
use jiff::{SignedDuration, Timestamp};
use skym_core::subject::{HostId, Subject, WorkloadKey};
use skym_core::view::{
    AppSummary, EndpointOverview, IncidentView, WorkloadSummary, workload_summary,
};
use std::collections::BTreeMap;

/// Generous bound on rows read for one response; lists are cut to `limit` afterwards.
pub const MAX_ROWS: usize = 10_000;

pub struct Snapshot {
    pub hosts: Vec<HostRow>,
    pub workloads: Vec<WorkloadRow>,
    /// Open incidents, muted ones included (mutes live in the configuration).
    pub open: Vec<Incident>,
    probes: BTreeMap<String, ProbeRow>,
    history: History,
    stopped: BTreeMap<WorkloadKey, Timestamp>,
    hosts_seen: BTreeMap<HostId, Timestamp>,
    urls_seen: BTreeMap<String, Timestamp>,
    pub now: Timestamp,
}

impl Snapshot {
    pub async fn read(s: &AppState) -> Result<Snapshot, ApiError> {
        let now = Timestamp::now();
        let (hosts, workloads, probes, open, history) = s
            .store
            .call(move |c| {
                let history = History {
                    last_deployed: history::last_deployed(c)?,
                    deploys: history::deploys(c, now - SignedDuration::from_hours(24 * 30))?,
                    exceptions_1h: history::application_exceptions(
                        c,
                        now - SignedDuration::from_hours(1),
                    )?,
                };
                Ok((
                    hosts::all(c)?,
                    hosts::all_workloads(c)?,
                    probes::all(c)?,
                    incidents::listed(c, true, None, None, Timestamp::UNIX_EPOCH, MAX_ROWS)?,
                    history,
                ))
            })
            .await?;
        Ok(Snapshot {
            stopped: workloads
                .iter()
                .filter_map(|w| Some((w.key.clone(), w.state.stopped_since()?)))
                .collect(),
            hosts_seen: hosts.iter().map(|h| (h.id.clone(), h.first_seen)).collect(),
            urls_seen: probes.iter().map(|p| (p.url.clone(), p.first_seen)).collect(),
            probes: probes.into_iter().map(|p| (p.url.clone(), p)).collect(),
            hosts,
            workloads,
            open,
            history,
            now,
        })
    }

    pub fn views(&self, s: &AppState, incidents: &[Incident]) -> Vec<IncidentView> {
        let cx = Context {
            mutes: &s.cfg.mute,
            stopped: &self.stopped,
            probed: &s.probed,
            hosts_seen: &self.hosts_seen,
            urls_seen: &self.urls_seen,
            now: self.now,
        };
        let open: Vec<IncidentView> = self.open.iter().map(|i| incident_view(i, &cx)).collect();
        // Each workload once, its status from every open incident (whatever was asked for).
        let workloads: BTreeMap<&WorkloadKey, WorkloadSummary> = self
            .workloads
            .iter()
            .map(|w| {
                let links = links(&Subject::Workload(w.key.clone()));
                (&w.key, workload_summary(w.key.clone(), w.facts.as_ref(), &w.state, &open, links))
            })
            .collect();
        // A workload's problem carries the workload, for a reader that looks no further.
        incidents
            .iter()
            .map(|i| {
                let mut v = incident_view(i, &cx);
                if let Subject::Workload(k) = &v.subject {
                    v.workload = workloads.get(k).cloned().map(Box::new);
                }
                v
            })
            .collect()
    }

    /// Every application, most urgent first; `open` are the views of `self.open`.
    pub fn apps(&self, s: &AppState, open: &[IncidentView]) -> Vec<AppSummary> {
        let endpoints: Vec<EndpointOverview> = s
            .probed
            .iter()
            .map(|e| {
                let row = self.probes.get(&e.url);
                views::endpoint(e, row, open, s.cfg.report_interval, self.now)
            })
            .collect();
        apps::summaries(&self.workloads, &s.cfg.hosts, &s.cfg.apps, &endpoints, open, &self.history)
    }
}
