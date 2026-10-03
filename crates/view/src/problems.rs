//! The problem list, the heart of the view: what is new, what has been going on, and since
//! when. Pure.

use jiff::{SignedDuration, Timestamp};
use skym_core::rules::Severity;
use skym_core::subject::Subject;
use skym_core::view::{IncidentView, Overview};

/// Incidents opened this close to when skym started watching were already there.
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

/// What an endpoint's problems show where a host's show the host.
pub const ENDPOINT: &str = "endpoint";

/// The problems of every host and endpoint, or of the one picked (`host:` or `endpoint:`);
/// muted ones come from `muted` when shown.
pub fn problems<'a>(
    overview: &'a Overview,
    muted: &'a [IncidentView],
    picked: Option<&Subject>,
    now: Timestamp,
) -> Problems<'a> {
    let shown = |s: Subject| picked.is_none_or(|p| *p == s);
    let customers = overview.customers.iter();
    let hosts =
        customers.clone().flat_map(|c| &c.hosts).filter(|h| shown(Subject::Host(h.id.clone())));
    let host_rows = hosts.flat_map(|h| {
        let own_muted = muted.iter().filter(|i| i.muted && i.subject.host() == Some(&h.id));
        h.incidents.iter().chain(own_muted).map(|i| (h.id.as_str(), i, h.observed_since))
    });
    let endpoints =
        customers.flat_map(|c| &c.endpoints).filter(|e| shown(Subject::Endpoint(e.url.clone())));
    let endpoint_rows = endpoints.flat_map(|e| {
        let subject = Subject::Endpoint(e.url.clone());
        let own_muted = muted.iter().filter(move |i| i.muted && i.subject == subject);
        e.incidents.iter().chain(own_muted).map(|i| (ENDPOINT, i, e.observed_since))
    });
    let mut out = Problems::default();
    for (host, incident, observed_since) in host_rows.chain(endpoint_rows) {
        let row = Row { host, incident, age: age(incident, observed_since, now) };
        let group = match incident.severity {
            Severity::Info => &mut out.info,
            _ if is_new(incident, observed_since, now) => &mut out.new,
            _ => &mut out.ongoing,
        };
        group.push(row);
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
