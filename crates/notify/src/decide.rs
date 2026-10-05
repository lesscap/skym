//! What to tell, from the incidents open now and what was announced. Pure.

use crate::state::{Announced, Id, Label, State};
use jiff::{SignedDuration, Timestamp};
use skym_core::rules::{IncidentCode, Severity};
use skym_core::subject::Subject;
use skym_core::view::IncidentView;

/// How long an announced incident must stay gone before it is announced resolved: one
/// that comes back meanwhile (flapping) says nothing.
pub const HOLD: SignedDuration = SignedDuration::from_mins(10);

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    New(Severity),
    Worse,
    Resolved,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Notice {
    pub kind: Kind,
    pub label: Label,
    pub code: IncidentCode,
    pub detail: Option<String>,
}

/// The notices of this pass and the state once they are delivered.
///
/// - New: open, not muted, worse than hygiene, not announced.
/// - Worse: announced below critical, now critical; said once.
/// - Resolved: announced, then missing from the open incidents for [`HOLD`].
///
/// `complete` is false when the list was cut short: nothing missing from it is resolved.
pub fn decide(
    open: &[IncidentView],
    complete: bool,
    state: &State,
    now: Timestamp,
) -> (Vec<Notice>, State) {
    let mut notices = Vec::new();
    let mut announced = Vec::new();
    for a in &state.announced {
        match open.iter().find(|i| id(i) == a.id) {
            Some(i) => {
                // While muted, it stays as announced: worse is told once the mute ends.
                let severity = if i.muted { a.severity } else { a.severity.max(i.severity) };
                if severity >= Severity::Critical && a.severity < Severity::Critical {
                    notices.push(notice(Kind::Worse, i));
                }
                announced.push(Announced { severity, absent_since: None, ..a.clone() });
            }
            None if !complete => announced.push(a.clone()),
            None => {
                let since = a.absent_since.unwrap_or(now);
                if now.duration_since(since) >= HOLD {
                    notices.push(Notice {
                        kind: Kind::Resolved,
                        label: a.label.clone(),
                        code: a.id.code,
                        detail: None,
                    });
                } else {
                    announced.push(Announced { absent_since: Some(since), ..a.clone() });
                }
            }
        }
    }
    let fresh = open.iter().filter(|i| {
        !i.muted && i.severity > Severity::Info && !state.announced.iter().any(|a| a.id == id(i))
    });
    for i in fresh {
        notices.push(notice(Kind::New(i.severity), i));
        announced.push(Announced {
            id: id(i),
            severity: i.severity,
            label: label(i),
            absent_since: None,
        });
    }
    notices.sort_by_key(|n| rank(&n.kind));
    (notices, State { announced })
}

/// Worst first: critical news, then warnings, then the resolved.
fn rank(kind: &Kind) -> u8 {
    match kind {
        Kind::New(Severity::Warn) => 1,
        Kind::New(_) | Kind::Worse => 0,
        Kind::Resolved => 2,
    }
}

fn id(i: &IncidentView) -> Id {
    Id { subject: i.subject.clone(), code: i.code, opened_at: i.opened_at }
}

fn notice(kind: Kind, i: &IncidentView) -> Notice {
    let detail = match (&i.subject, &i.app) {
        // A project's service is not in its label; a lone container's is.
        (Subject::Workload(k), Some(app)) if app.service.is_none() => {
            format!("{}: {}", k.service, i.detail)
        }
        _ => i.detail.clone(),
    };
    Notice { kind, label: label(i), code: i.code, detail: Some(detail) }
}

/// The host, and the application it belongs to, else what it is about.
fn label(i: &IncidentView) -> Label {
    let host = i.subject.host().or(i.app.as_ref().map(|a| &a.host));
    let what = match (&i.app, &i.subject) {
        (Some(app), _) => app.label().to_string(),
        (None, Subject::Mount { path, .. }) => path.clone(),
        (None, Subject::Host(_)) => "host".into(),
        (None, Subject::Endpoint(url)) => url.clone(),
        (None, Subject::Workload(k)) => format!("{}/{}", k.project, k.service),
        (None, s) => s.to_string(),
    };
    Label { host: host.cloned().unwrap_or_else(|| "-".into()), what }
}

#[cfg(test)]
mod tests {
    use super::*;
    use skym_core::fixtures::incident;
    use skym_core::rules::IncidentCode::{DiskFilling, WorkloadDown};
    use skym_core::rules::Severity::{Critical, Info, Warn};

    fn at(minutes: i64) -> Timestamp {
        Timestamp::from_second(1_790_000_000 + minutes * 60).unwrap()
    }

    fn open(subject: &str, code: IncidentCode, severity: Severity) -> IncidentView {
        IncidentView {
            opened_at: Some(at(0)),
            detail: "it broke".into(),
            ..incident(subject, code, severity)
        }
    }

    fn kinds(notices: &[Notice]) -> Vec<Kind> {
        notices.iter().map(|n| n.kind.clone()).collect()
    }

    #[test]
    fn the_first_run_announces_what_is_open_worst_first_and_never_hygiene_or_muted() {
        let muted = IncidentView { muted: true, ..open("host:y", WorkloadDown, Critical) };
        let incidents = [
            open("mount:x:/data", DiskFilling, Warn),
            open("host:x", WorkloadDown, Critical),
            open("mount:i:/", DiskFilling, Info),
            muted,
        ];
        let (notices, state) = decide(&incidents, true, &State::default(), at(1));
        assert_eq!(kinds(&notices), [Kind::New(Critical), Kind::New(Warn)]);
        assert_eq!(state.announced.len(), 2);
        let (again, _) = decide(&incidents, true, &state, at(2));
        assert!(again.is_empty(), "announced once");
    }

    #[test]
    fn a_muted_incident_is_announced_once_its_mute_ends() {
        let muted = IncidentView { muted: true, ..open("host:x", WorkloadDown, Warn) };
        let (none, state) = decide(std::slice::from_ref(&muted), true, &State::default(), at(1));
        assert!(none.is_empty() && state.announced.is_empty());
        let unmuted = IncidentView { muted: false, ..muted };
        let (notices, state) = decide(std::slice::from_ref(&unmuted), true, &state, at(2));
        assert_eq!(kinds(&notices), [Kind::New(Warn)]);
        let muted_again = IncidentView { muted: true, ..unmuted };
        let (none, state) = decide(&[muted_again], true, &state, at(3));
        assert!(none.is_empty() && state.announced.len() == 1, "still announced, resolved later");
    }

    #[test]
    fn getting_worse_is_told_once_however_it_oscillates() {
        let warn = open("host:x", WorkloadDown, Warn);
        let critical = IncidentView { severity: Critical, ..warn.clone() };
        let (_, state) = decide(std::slice::from_ref(&warn), true, &State::default(), at(0));
        let (notices, state) = decide(std::slice::from_ref(&critical), true, &state, at(1));
        assert_eq!(kinds(&notices), [Kind::Worse]);
        let (none, state) = decide(&[warn], true, &state, at(2));
        assert!(none.is_empty(), "better is not told");
        let (none, _) = decide(&[critical], true, &state, at(3));
        assert!(none.is_empty(), "worse again is not told twice");

        let warn = open("host:y", WorkloadDown, Warn);
        let unknown = IncidentView { severity: Severity::Unknown, ..warn.clone() };
        let (_, state) = decide(&[warn], true, &State::default(), at(0));
        let (notices, _) = decide(&[unknown], true, &state, at(1));
        assert_eq!(kinds(&notices), [Kind::Worse], "unknown ranks above critical");
    }

    #[test]
    fn a_muted_incident_getting_worse_is_told_when_its_mute_ends() {
        let warn = open("host:x", WorkloadDown, Warn);
        let muted = IncidentView { muted: true, severity: Critical, ..warn.clone() };
        let (_, state) = decide(&[warn], true, &State::default(), at(0));
        let (none, state) = decide(std::slice::from_ref(&muted), true, &state, at(1));
        assert!(none.is_empty(), "muted says nothing");
        let unmuted = IncidentView { muted: false, ..muted };
        let (notices, _) = decide(&[unmuted], true, &state, at(2));
        assert_eq!(kinds(&notices), [Kind::Worse]);
    }

    #[test]
    fn resolved_after_ten_minutes_gone_and_nothing_when_it_comes_back_before() {
        let i = open("host:x", WorkloadDown, Critical);
        let (_, state) = decide(std::slice::from_ref(&i), true, &State::default(), at(0));
        let (none, state) = decide(&[], true, &state, at(1));
        assert!(none.is_empty());
        let (none, back) = decide(std::slice::from_ref(&i), true, &state, at(5));
        assert!(none.is_empty(), "flapping says nothing");
        let (none, state) = decide(&[], true, &back, at(6));
        assert!(none.is_empty());
        let (none, state) = decide(&[], true, &state, at(15));
        assert!(none.is_empty(), "absence counts from its return, not from the first");
        let (notices, state) = decide(&[], true, &state, at(16));
        assert_eq!(kinds(&notices), [Kind::Resolved]);
        assert_eq!(notices[0].label, Label { host: "x".into(), what: "host".into() });
        assert!(state.announced.is_empty());
    }

    #[test]
    fn a_cut_short_list_resolves_nothing() {
        let i = open("host:x", WorkloadDown, Critical);
        let (_, state) = decide(&[i], true, &State::default(), at(0));
        let (none, kept) = decide(&[], false, &state, at(1));
        assert!(none.is_empty());
        assert_eq!(kept, state, "absence is not even counted");
        let (none, _) = decide(&[], false, &kept, at(60));
        assert!(none.is_empty());
    }

    #[test]
    fn a_reopened_incident_is_a_new_one() {
        let first = open("host:x", WorkloadDown, Critical);
        let (_, state) = decide(&[first], true, &State::default(), at(0));
        let second =
            IncidentView { opened_at: Some(at(40)), ..open("host:x", WorkloadDown, Critical) };
        let (notices, _) = decide(&[second], true, &state, at(41));
        assert_eq!(kinds(&notices), [Kind::New(Critical)]);
    }

    #[test]
    fn names_are_those_of_skym_view() {
        let label = |subject: &str, app: Option<&str>| {
            let i = IncidentView {
                app: app.map(|a| a.parse().unwrap()),
                ..open(subject, WorkloadDown, Warn)
            };
            let n = notice(Kind::New(Warn), &i);
            (n.label.host, n.label.what, n.detail.unwrap())
        };
        let s = String::from;
        assert_eq!(
            label("workload:x/shop/api", Some("x/shop")),
            (s("x"), s("shop"), s("api: it broke"))
        );
        assert_eq!(
            label("workload:x/-/caddy", Some("x/-/caddy")),
            (s("x"), s("caddy"), s("it broke"))
        );
        assert_eq!(label("mount:yi1:/data", None), (s("yi1"), s("/data"), s("it broke")));
        assert_eq!(label("host:i", None), (s("i"), s("host"), s("it broke")));
        assert_eq!(
            label("endpoint:https://a.example", None),
            (s("-"), s("https://a.example"), s("it broke"))
        );
        assert_eq!(
            label("endpoint:https://a.example", Some("external/pay")),
            (s("external"), s("pay"), s("it broke"))
        );
        assert_eq!(label("workload:x/shop/api", None), (s("x"), s("shop/api"), s("it broke")));
    }
}
