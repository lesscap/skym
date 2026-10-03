//! The problem list, the heart of the view: what is new, what has been going on, and since
//! when. Pure.

use jiff::{SignedDuration, Timestamp};
use skym_core::rules::Severity;
use skym_core::view::{IncidentView, Overview};

/// Incidents opened this close to when skym started watching a host were already there.
const BASELINE: SignedDuration = SignedDuration::from_mins(5);
const NEW_FOR: SignedDuration = SignedDuration::from_hours(24);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Age {
    /// Known: the problem began this long ago.
    Exact(SignedDuration),
    /// It predates skym: at least this long.
    AtLeast(SignedDuration),
}

impl Age {
    fn length(self) -> SignedDuration {
        match self {
            Age::Exact(d) | Age::AtLeast(d) => d,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row<'a> {
    pub host: &'a str,
    pub incident: &'a IncidentView,
    pub age: Age,
}

#[derive(Debug, Default, PartialEq)]
pub struct Problems<'a> {
    pub new: Vec<Row<'a>>,
    pub ongoing: Vec<Row<'a>>,
    /// Hygiene (`info`), shown folded unless asked for.
    pub info: Vec<Row<'a>>,
}

/// The problems of every host, or of one; muted ones come from `muted` when shown.
pub fn problems<'a>(
    overview: &'a Overview,
    muted: &'a [IncidentView],
    host: Option<&str>,
    now: Timestamp,
) -> Problems<'a> {
    let hosts = overview.customers.iter().flat_map(|c| &c.hosts);
    let mut out = Problems::default();
    for h in hosts.filter(|h| host.is_none_or(|id| id == h.id)) {
        let own_muted = muted.iter().filter(|i| i.muted && i.subject.host() == Some(&h.id));
        for incident in h.incidents.iter().chain(own_muted) {
            let row = Row { host: &h.id, incident, age: age(incident, h.observed_since, now) };
            let group = match incident.severity {
                Severity::Info => &mut out.info,
                _ if is_new(incident, h.observed_since, now) => &mut out.new,
                _ => &mut out.ongoing,
            };
            group.push(row);
        }
    }
    for group in [&mut out.new, &mut out.ongoing, &mut out.info] {
        // Worst first, then longest-standing, then by name for a stable list.
        group.sort_by(|a, b| {
            (b.incident.severity.cmp(&a.incident.severity))
                .then(b.age.length().cmp(&a.age.length()))
                .then(a.host.cmp(b.host))
                .then(a.incident.subject.cmp(&b.incident.subject))
        });
    }
    out
}

/// Opened after skym had its baseline of the host, within the last day.
fn is_new(i: &IncidentView, observed_since: Option<Timestamp>, now: Timestamp) -> bool {
    match (i.opened_at, observed_since) {
        (Some(opened), Some(watched)) => opened >= watched + BASELINE && now - NEW_FOR < opened,
        _ => false,
    }
}

/// The real start when known; else, for a problem skym found already there, at least as
/// long as skym has watched; else since it opened.
pub fn age(i: &IncidentView, observed_since: Option<Timestamp>, now: Timestamp) -> Age {
    let since = |t: Timestamp| now.duration_since(t);
    match (i.since, i.opened_at, observed_since) {
        (Some(began), ..) => Age::Exact(since(began)),
        (None, Some(opened), Some(watched)) if opened < watched + BASELINE => {
            Age::AtLeast(since(watched))
        }
        (None, Some(opened), _) => Age::Exact(since(opened)),
        (None, None, _) => Age::Exact(SignedDuration::ZERO),
    }
}

#[cfg(test)]
#[path = "problems_tests.rs"]
mod tests;
