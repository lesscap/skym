//! What the screens list, derived from the data: hosts, problems, services.

use super::{App, Pane, Screen};
use crate::apps::{self, Group, groups};
use crate::names;
use crate::problems::{Problems, Row, problems};
use jiff::Timestamp;
use skym_core::subject::Subject;
use skym_core::view::{AppSummary, Status, WorkloadSummary};

impl App {
    /// What the left pane lists, in the overview's order: each customer's hosts, then its
    /// endpoints, most urgent first.
    pub fn targets(&self) -> Vec<Subject> {
        let customers = self.overview.value.iter().flat_map(|o| &o.customers);
        let all = customers.flat_map(|c| {
            let hosts = c.hosts.iter().map(|h| Subject::Host(h.id.clone()));
            hosts.chain(c.endpoints.iter().map(|e| Subject::Endpoint(e.url.clone())))
        });
        all.filter(|s| !self.filtering(Pane::Hosts) || self.matches(&names::target(s))).collect()
    }

    /// When skym started watching a host, from the overview.
    pub fn observed_since(&self, host: &str) -> Option<Timestamp> {
        let customers = self.overview.value.iter().flat_map(|o| &o.customers);
        customers.flat_map(|c| &c.hosts).find(|h| h.id == host).and_then(|h| h.observed_since)
    }

    /// The filter applies to the list in view: on the overview, the focused pane's.
    fn filtering(&self, pane: Pane) -> bool {
        self.frame().screen == Screen::Overview && self.pane == pane
    }

    /// The host or endpoint picked in the overview's left pane, if not "All hosts".
    pub fn picked(&self) -> Option<Subject> {
        self.host_cursor.checked_sub(1).and_then(|i| self.targets().into_iter().nth(i))
    }

    /// The overview's problems for the picked host, filtered by name.
    pub fn problem_groups(&self, now: Timestamp) -> Problems<'_> {
        let Some(overview) = &self.overview.value else { return Problems::default() };
        let muted = match (&self.muted.value, self.show_muted) {
            (Some(list), true) => list.incidents.as_slice(),
            _ => &[],
        };
        let mut p = problems(overview, muted, self.picked().as_ref(), now);
        if self.filtering(Pane::Problems) {
            for group in [&mut p.new, &mut p.ongoing, &mut p.info] {
                group.retain(|r| {
                    self.matches(&format!("{} {}", r.host, names::full(&r.incident.subject)))
                });
            }
        }
        p
    }

    /// The overview's problem rows as listed: new, ongoing, then hygiene when shown.
    pub fn problem_rows(&self, now: Timestamp) -> Vec<Row<'_>> {
        let p = self.problem_groups(now);
        let info = if self.show_info { p.info } else { Vec::new() };
        p.new.into_iter().chain(p.ongoing).chain(info).collect()
    }

    /// The services of the host or application in view: those with problems first.
    pub fn services(&self) -> Vec<&WorkloadSummary> {
        let all = match &self.frame().screen {
            Screen::App(_) => self.app.value.iter().flat_map(|a| &a.workloads).collect::<Vec<_>>(),
            _ => self.host.value.iter().flat_map(|h| &h.workloads).collect(),
        };
        let mut list: Vec<&WorkloadSummary> = all
            .into_iter()
            .filter(|w| self.matches(&names::full(&Subject::Workload(w.key.clone()))))
            .collect();
        list.sort_by(|a, b| {
            let worse = |w: &WorkloadSummary| w.status != Status::Ok;
            worse(b).cmp(&worse(a)).then(b.status.cmp(&a.status)).then(a.key.cmp(&b.key))
        });
        list
    }

    /// The applications page's groups, filtered by name, host, URL or note.
    pub fn app_groups(&self) -> Vec<Group<'_>> {
        let list = self.apps.value.as_ref().map_or(&[][..], |l| l.apps.as_slice());
        groups(list, self.all_envs, |a| self.matches(&apps::text(a)))
    }

    /// The applications as listed, for moving the selection.
    pub fn app_rows(&self) -> Vec<&AppSummary> {
        self.app_groups().into_iter().flat_map(|g| g.apps).collect()
    }

    /// Keeps the picked host within the host list as it shrinks.
    pub(super) fn clamp_host_cursor(&mut self) {
        self.host_cursor = self.host_cursor.min(self.targets().len());
    }

    fn matches(&self, name: &str) -> bool {
        self.filter.as_deref().is_none_or(|f| name.to_lowercase().contains(&f.to_lowercase()))
    }

    /// How many rows the current list has (for moving the selection).
    pub(super) fn rows(&self, now: Timestamp) -> usize {
        match (&self.frame().screen, self.pane) {
            (Screen::Overview, Pane::Hosts) => self.targets().len() + 1,
            (Screen::Overview, Pane::Problems) => self.problem_rows(now).len(),
            (Screen::Host(_) | Screen::App(_), _) => self.services().len(),
            (Screen::Apps, _) => self.app_rows().len(),
            (Screen::Workload(_), _) => {
                self.workload.value.as_ref().map_or(0, |w| w.exceptions.len())
            }
            (Screen::Exceptions(_), _) => {
                self.exceptions.value.as_ref().map_or(0, |e| e.exceptions.len())
            }
            (Screen::Timeline { .. }, _) => {
                self.timeline.value.as_ref().map_or(0, |t| t.entries.len())
            }
        }
    }
}
