//! What the screens list, derived from the data: problems, applications, hosts, services.

use super::{App, Screen};
use crate::apps::{self, Group, groups};
use crate::names;
use crate::problems::{Problems, Row, problems};
use jiff::Timestamp;
use skym_core::subject::{AppKey, Subject};
use skym_core::view::{AppSummary, HostOverview, Status, WorkloadSummary};

impl App {
    /// The problems tab, filtered by app, host, what and why.
    pub fn problem_groups(&self, now: Timestamp) -> Problems<'_> {
        let Some(overview) = &self.overview.value else { return Problems::default() };
        let muted = match (&self.muted.value, self.show_muted) {
            (Some(list), true) => list.incidents.as_slice(),
            _ => &[],
        };
        let mut p = problems(overview, muted, now);
        for group in [&mut p.new, &mut p.ongoing, &mut p.info] {
            group.retain(|r| {
                let i = r.incident;
                let what = names::what(&i.subject);
                self.matches(&format!("{} {} {what} {}", self.app_name(r), r.host, i.detail))
            });
        }
        p
    }

    /// The problem rows as listed: new, ongoing, then hygiene when shown.
    pub fn problem_rows(&self, now: Timestamp) -> Vec<Row<'_>> {
        let p = self.problem_groups(now);
        let info = if self.show_info { p.info } else { Vec::new() };
        p.new.into_iter().chain(p.ongoing).chain(info).collect()
    }

    /// What a problem row calls its application: its configured name, or `host <id>` for a
    /// host's own problem.
    pub fn app_name(&self, r: &Row) -> String {
        match r.app() {
            Some(key) => {
                self.app_summary(key).map_or_else(|| key.label().to_string(), |a| a.name.clone())
            }
            None => format!("host {}", r.host),
        }
    }

    fn app_summary(&self, key: &AppKey) -> Option<&AppSummary> {
        self.apps.value.iter().flat_map(|l| &l.apps).find(|a| a.key == *key)
    }

    /// The hosts tab: most urgent first, filtered by name.
    pub fn host_rows(&self) -> Vec<&HostOverview> {
        let hosts = self.overview.value.iter().flat_map(|o| &o.hosts);
        hosts.filter(|h| self.matches(&h.id)).collect()
    }

    /// The applications tab's groups, filtered by name, host, URL or note.
    pub fn app_groups(&self) -> Vec<Group<'_>> {
        let list = self.apps.value.as_ref().map_or(&[][..], |l| l.apps.as_slice());
        groups(list, self.all_envs, |a| self.matches(&apps::text(a)))
    }

    /// The applications as listed, for moving the selection.
    pub fn app_rows(&self) -> Vec<&AppSummary> {
        self.app_groups().into_iter().flat_map(|g| g.apps).collect()
    }

    /// A host's applications (the server lists those with problems first), filtered.
    pub fn host_apps(&self) -> Vec<&AppSummary> {
        let apps = self.host.value.iter().flat_map(|h| &h.apps);
        apps.filter(|a| self.matches(&apps::text(a))).collect()
    }

    /// An application's services: those with problems first, then by name.
    pub fn services(&self) -> Vec<&WorkloadSummary> {
        let mut list: Vec<&WorkloadSummary> = self
            .app
            .value
            .iter()
            .flat_map(|a| &a.workloads)
            .filter(|w| self.matches(&names::full(&Subject::Workload(w.key.clone()))))
            .collect();
        list.sort_by(|a, b| {
            let worse = |w: &WorkloadSummary| w.status != Status::Ok;
            worse(b).cmp(&worse(a)).then(b.status.cmp(&a.status)).then(a.key.cmp(&b.key))
        });
        list
    }

    fn matches(&self, name: &str) -> bool {
        self.filter.as_deref().is_none_or(|f| name.to_lowercase().contains(&f.to_lowercase()))
    }

    /// How many rows the current list has (for moving the selection).
    pub(super) fn rows(&self, now: Timestamp) -> usize {
        match &self.frame().screen {
            Screen::Problems => self.problem_rows(now).len(),
            Screen::Apps => self.app_rows().len(),
            Screen::Hosts => self.host_rows().len(),
            Screen::Host(_) => self.host_apps().len(),
            Screen::App(_) => self.services().len(),
            Screen::Workload(_) => self.workload.value.as_ref().map_or(0, |w| w.exceptions.len()),
            Screen::Exceptions(_) => {
                self.exceptions.value.as_ref().map_or(0, |e| e.exceptions.len())
            }
            Screen::Timeline { .. } => self.timeline.value.as_ref().map_or(0, |t| t.entries.len()),
        }
    }
}
