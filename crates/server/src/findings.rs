//! Findings only the server can make: lost heartbeats, disk projection, probes and
//! applications gone missing.

use crate::config::AppConfig;
use crate::probe::Probe;
use jiff::{SignedDuration, Timestamp};
use skym_core::report::Report;
use skym_core::rules::{Finding, IncidentCode, Severity};
use skym_core::subject::{HostId, Subject};
use skym_core::time::format_duration;
use std::collections::BTreeMap;

/// Hosts silent for more than three intervals. Silence is counted from the server's start
/// at the earliest, so a server restart does not declare every host lost. Hosts that never
/// reported have no heartbeat to lose (their status is unknown).
pub fn heartbeat(
    hosts: &[(HostId, Option<Timestamp>)],
    started: Timestamp,
    interval: SignedDuration,
    now: Timestamp,
) -> Vec<Finding> {
    let silent: Vec<(&HostId, SignedDuration)> = hosts
        .iter()
        .filter_map(|(h, seen)| Some((h, now.duration_since((*seen)?.max(started)))))
        .filter(|(_, age)| *age > interval * 3)
        .collect();
    let reported = hosts.iter().filter(|(_, seen)| seen.is_some()).count();
    let all_silent = silent.len() >= 2 && silent.len() * 5 >= reported * 4;
    let prefix = if all_silent { "all hosts silent, likely server side or network: " } else { "" };
    silent
        .into_iter()
        .map(|(host, age)| Finding {
            subject: Subject::Host(host.clone()),
            code: IncidentCode::HeartbeatLost,
            severity: Severity::Critical,
            detail: format!("{prefix}no report for {}", format_duration(age)),
        })
        .collect()
}

/// Right after a server start, a host last seen before it cannot be judged yet: its
/// silence only counts from the start, so an existing incident is neither confirmed nor cleared.
pub fn in_grace(
    last_seen: Option<Timestamp>,
    started: Timestamp,
    interval: SignedDuration,
    now: Timestamp,
) -> bool {
    last_seen.is_some_and(|t| t < started) && now.duration_since(started) <= interval * 3
}

/// When a mount fills up at its recent rate: a least-squares slope over the samples
/// (at least 30 spanning 5 hours). Open incidents stay matched up to 7 days out.
pub fn projection(
    samples: &[(Timestamp, u64)],
    total: u64,
    open: bool,
) -> Option<(Severity, SignedDuration)> {
    let (first, last) = (samples.first()?, samples.last()?);
    if samples.len() < 30 || last.0.duration_since(first.0) < SignedDuration::from_hours(5) {
        return None;
    }
    let points: Vec<(f64, f64)> = samples
        .iter()
        .map(|(t, used)| (t.duration_since(first.0).as_secs_f64(), *used as f64))
        .collect();
    // Least squares: centring x alone is enough, since Σ(x − x̄) = 0.
    let mx = points.iter().map(|p| p.0).sum::<f64>() / points.len() as f64;
    let (sxy, sxx) = points
        .iter()
        .fold((0.0, 0.0), |(sxy, sxx), (x, y)| (sxy + (x - mx) * y, sxx + (x - mx) * x));
    let slope = sxy / sxx;
    if !slope.is_finite() || slope <= 0.0 {
        return None;
    }
    // A nearly flat disk projects beyond what a duration holds: far beyond a week, so none.
    let full_in =
        SignedDuration::try_from_secs_f64(total.saturating_sub(last.1) as f64 / slope).ok()?;
    let (week, day) = (SignedDuration::from_hours(24 * 7), SignedDuration::from_hours(24));
    let matched = if open { full_in <= week } else { full_in < week };
    matched.then(|| (if full_in < day { Severity::Critical } else { Severity::Warn }, full_in))
}

/// What one probe says: an endpoint down or answering otherwise than expected, and a
/// certificate close to its expiry.
pub fn endpoint(url: &str, expect: &[u16], p: &Probe) -> Vec<Finding> {
    let finding = |code, (severity, detail)| Finding {
        subject: Subject::Endpoint(url.to_string()),
        code,
        severity,
        detail,
    };
    let down = match &p.response {
        Err(why) => Some((Severity::Critical, why.clone())),
        Ok(status) => answer(*status, expect),
    };
    let cert = p.cert_not_after.and_then(|t| expiring(t, p.at));
    down.map(|d| finding(IncidentCode::EndpointDown, d))
        .into_iter()
        .chain(cert.map(|c| finding(IncidentCode::CertExpiring, c)))
        .collect()
}

/// `None` when the status is the one expected (by default, anything below 400).
fn answer(status: u16, expect: &[u16]) -> Option<(Severity, String)> {
    let good = if expect.is_empty() { status < 400 } else { expect.contains(&status) };
    if good {
        return None;
    }
    let reason = reqwest::StatusCode::from_u16(status).ok().and_then(|s| s.canonical_reason());
    let mut detail = reason.map_or_else(|| status.to_string(), |r| format!("{status} {r}"));
    if !expect.is_empty() {
        let expected: Vec<String> = expect.iter().map(u16::to_string).collect();
        detail = format!("{detail}, expected {}", expected.join(" or "));
    }
    let severity = if status >= 500 { Severity::Critical } else { Severity::Warn };
    Some((severity, detail))
}

fn expiring(not_after: Timestamp, now: Timestamp) -> Option<(Severity, String)> {
    let left = not_after.duration_since(now);
    let day = SignedDuration::from_hours(24);
    if left > day * 14 {
        return None;
    }
    let date = not_after.strftime("%Y-%m-%d");
    let detail = if left.is_negative() {
        format!("certificate expired {date}")
    } else {
        format!("certificate expires {date} (in {})", format_duration(left))
    };
    Some((if left <= day * 7 { Severity::Critical } else { Severity::Warn }, detail))
}

/// Configured container applications of the reporting host with nothing in the report, not
/// even a stopped container (that is `WORKLOAD_DOWN`), when the agent listed every container.
/// systemd units are declared to the agent, so a missing one is `WORKLOAD_DOWN` too.
/// Critical in `prod`, warn elsewhere.
pub fn missing_apps(r: &Report, apps: &[AppConfig]) -> Vec<Finding> {
    if !r.containers_listed {
        return Vec::new();
    }
    apps.iter()
        .filter(|a| a.id.host == r.host && !a.id.is_unit())
        .filter(|a| !r.workloads.iter().any(|w| a.id.contains(&w.key)))
        .map(|a| Finding {
            subject: Subject::App(a.id.clone()),
            code: IncidentCode::AppMissing,
            severity: if a.env.as_deref() == Some("prod") {
                Severity::Critical
            } else {
                Severity::Warn
            },
            detail: match &a.id.service {
                Some(container) => format!("no container {container} on {}", r.host),
                None => format!("no container of {} on {}", a.id.project, r.host),
            },
        })
        .collect()
}

/// One finding per `(subject, code)`: the highest severity, details joined.
pub fn merge(findings: Vec<Finding>) -> Vec<Finding> {
    findings
        .into_iter()
        .fold(BTreeMap::<(Subject, IncidentCode), Finding>::new(), |mut acc, f| {
            match acc.get_mut(&(f.subject.clone(), f.code)) {
                Some(kept) => {
                    kept.severity = kept.severity.max(f.severity);
                    kept.detail = format!("{}; {}", kept.detail, f.detail);
                }
                None => {
                    acc.insert((f.subject.clone(), f.code), f);
                }
            }
            acc
        })
        .into_values()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: i64) -> Timestamp {
        Timestamp::from_second(1_790_000_000 + secs).unwrap()
    }

    #[test]
    fn heartbeat_is_lost_after_three_intervals_counted_from_server_start() {
        let interval = SignedDuration::from_secs(60);
        let hosts = [("x".to_string(), Some(at(0))), ("y".to_string(), None)];
        let lost = |now, started| heartbeat(&hosts, at(started), interval, at(now));
        assert!(lost(180, 0).is_empty(), "exactly three intervals is not yet lost");
        assert_eq!(lost(181, 0).len(), 1);
        assert!(lost(181, 100).is_empty(), "the server only started at 100");
        assert_eq!(lost(181, 0)[0].subject, Subject::Host("x".into()));
        assert!(!lost(181, 0)[0].detail.starts_with("all hosts"), "one host is not all of them");
    }

    #[test]
    fn many_hosts_silent_at_once_point_at_the_server_side() {
        let interval = SignedDuration::from_secs(60);
        let hosts: Vec<_> = ["a", "b", "c", "d", "e"]
            .iter()
            .enumerate()
            .map(|(i, h)| (h.to_string(), Some(at(if i == 0 { 1000 } else { 0 }))))
            .collect();
        let found = heartbeat(&hosts, at(0), interval, at(1000));
        assert_eq!(found.len(), 4);
        assert!(found.iter().all(|f| f.detail.starts_with("all hosts silent")), "4 of 5 is 80%");
        let three: Vec<_> = hosts[..3]
            .iter()
            .map(|(h, _)| (h.clone(), Some(at(0))))
            .chain(hosts[3..].iter().map(|(h, _)| (h.clone(), Some(at(1000)))))
            .collect();
        let found = heartbeat(&three, at(0), interval, at(1000));
        assert!(found.iter().all(|f| !f.detail.starts_with("all hosts")), "3 of 5 is not");
    }

    #[test]
    fn hosts_seen_before_a_restart_wait_out_the_grace_period() {
        let interval = SignedDuration::from_secs(60);
        let grace = |seen: Option<i64>, now| in_grace(seen.map(at), at(1000), interval, at(now));
        assert!(grace(Some(0), 1180), "seen before the start, three intervals not yet over");
        assert!(!grace(Some(0), 1181));
        assert!(!grace(Some(1000), 1100), "seen since the start");
        assert!(!grace(None, 1100), "never seen");
    }

    /// `n` samples, one per `step` seconds, growing by `rate` bytes per second.
    fn growing(n: i64, step: i64, rate: u64) -> Vec<(Timestamp, u64)> {
        (0..n).map(|i| (at(i * step), 1_000_000 + rate * (i * step) as u64)).collect()
    }

    #[test]
    fn projection_needs_enough_history() {
        // Fast growth, so only the history guard can say no.
        let p = |n, step| projection(&growing(n, step, 1_000), 100_000_000, false);
        assert_eq!(p(29, 1_000), None, "29 samples");
        assert!(p(30, 1_000).is_some(), "30 samples over 8h");
        assert_eq!(p(30, 600), None, "30 samples under 5h");
        assert!(p(31, 600).is_some(), "exactly 5h");
        assert_eq!(projection(&growing(40, 600, 0), 100_000_000, false), None, "flat disk");
    }

    #[test]
    fn a_nearly_flat_disk_projects_nothing_instead_of_overflowing() {
        // One byte more over eight hours on a huge disk: full in ~10^23 seconds.
        let mut samples: Vec<(Timestamp, u64)> =
            (0..40).map(|i| (at(i * 720), 1_000_000)).collect();
        samples.last_mut().unwrap().1 += 1;
        assert_eq!(projection(&samples, u64::MAX, false), None);
        assert_eq!(projection(&samples, u64::MAX, true), None);
    }

    #[test]
    fn projection_severity_and_hysteresis() {
        let samples = growing(40, 600, 1_000); // last used ≈ 24.4 MB, growing 1 KB/s
        let last = samples.last().unwrap().1;
        let full_in = |secs: u64| last + 1_000 * secs;
        let (sev, eta) = projection(&samples, full_in(3_600), false).unwrap();
        assert_eq!((sev, eta.as_secs() / 60), (Severity::Critical, 60));
        assert_eq!(projection(&samples, full_in(86_400 * 2), false).unwrap().0, Severity::Warn);
        assert_eq!(
            projection(&samples, full_in(86_400), false).unwrap().0,
            Severity::Warn,
            "exactly a day"
        );
        assert_eq!(projection(&samples, full_in(86_400 * 7), false), None);
        assert!(
            projection(&samples, full_in(86_400 * 7), true).is_some(),
            "an open incident holds at 7 days"
        );
    }

    fn probe(response: Result<u16, &str>, cert_in_days: Option<i64>) -> Probe {
        Probe {
            at: at(0),
            response: response.map_err(str::to_string),
            latency_ms: 40,
            cert_not_after: cert_in_days.map(|d| at(d * 86_400)),
        }
    }

    fn judged(expect: &[u16], p: &Probe) -> Vec<(IncidentCode, Severity, String)> {
        endpoint("https://shop.example.com", expect, p)
            .into_iter()
            .map(|f| (f.code, f.severity, f.detail))
            .collect()
    }

    #[test]
    fn an_endpoint_is_down_on_errors_and_unexpected_statuses() {
        let down = |sev, detail: &str| vec![(IncidentCode::EndpointDown, sev, detail.to_string())];
        assert_eq!(judged(&[], &probe(Ok(200), None)), []);
        assert_eq!(judged(&[], &probe(Ok(302), None)), [], "a redirect is an answer");
        assert_eq!(judged(&[], &probe(Ok(399), None)), []);
        assert_eq!(judged(&[], &probe(Ok(400), None)), down(Severity::Warn, "400 Bad Request"));
        assert_eq!(judged(&[], &probe(Ok(404), None)), down(Severity::Warn, "404 Not Found"));
        assert_eq!(judged(&[], &probe(Ok(499), None)), down(Severity::Warn, "499"));
        assert_eq!(
            judged(&[], &probe(Ok(500), None)),
            down(Severity::Critical, "500 Internal Server Error")
        );
        assert_eq!(
            judged(&[], &probe(Err("timeout after 10s"), None)),
            down(Severity::Critical, "timeout after 10s")
        );
    }

    #[test]
    fn expect_replaces_the_default() {
        assert_eq!(judged(&[401], &probe(Ok(401), None)), []);
        assert_eq!(
            judged(&[401, 403], &probe(Ok(200), None)),
            [(IncidentCode::EndpointDown, Severity::Warn, "200 OK, expected 401 or 403".into())]
        );
        assert_eq!(judged(&[401], &probe(Ok(503), None))[0].1, Severity::Critical);
    }

    #[test]
    fn certificates_warn_two_weeks_ahead_and_turn_critical_in_the_last_week() {
        let cert = |days| judged(&[], &probe(Ok(200), Some(days)));
        let at_day = |d: i64| at(d * 86_400).strftime("%Y-%m-%d").to_string();
        assert_eq!(cert(15), []);
        assert_eq!(
            cert(14),
            [(
                IncidentCode::CertExpiring,
                Severity::Warn,
                format!("certificate expires {} (in 14d)", at_day(14))
            )]
        );
        assert_eq!(cert(8)[0].1, Severity::Warn);
        assert_eq!(cert(7)[0].1, Severity::Critical);
        assert_eq!(
            cert(-1),
            [(
                IncidentCode::CertExpiring,
                Severity::Critical,
                format!("certificate expired {}", at_day(-1))
            )]
        );
        let both = judged(&[], &probe(Ok(503), Some(1)));
        assert_eq!(both.len(), 2, "down and expiring are separate incidents");
    }

    use crate::config::app_config as app;

    #[test]
    fn container_apps_go_missing_only_from_a_complete_listing() {
        let mut r = skym_core::fixtures::full_report(); // x: captain/api, pg/main, _systemd/xray
        r.workloads[0].state.run = skym_core::model::RunState::Exited;
        let apps = [
            app("x/captain", Some("prod")),
            app("x/shop", Some("prod")),
            app("x/-/redis", Some("test")),
            app("x/_systemd/nginx", Some("prod")),
            app("y/shop", Some("prod")),
        ];
        assert!(missing_apps(&r, &apps).is_empty(), "no complete listing, no judgement");
        r.containers_listed = true;
        let found: Vec<(String, Severity, String)> = missing_apps(&r, &apps)
            .into_iter()
            .map(|f| (f.subject.to_string(), f.severity, f.detail))
            .collect();
        assert_eq!(
            found,
            [
                ("app:x/shop".into(), Severity::Critical, "no container of shop on x".into()),
                ("app:x/-/redis".into(), Severity::Warn, "no container redis on x".into()),
            ],
            "an exited container is still there; a unit is WORKLOAD_DOWN's; other hosts judge theirs"
        );
    }

    #[test]
    fn merge_keeps_one_finding_per_subject_and_code() {
        let f = |sev, detail: &str| Finding {
            subject: Subject::Mount { host: "x".into(), path: "/".into() },
            code: IncidentCode::DiskFilling,
            severity: sev,
            detail: detail.into(),
        };
        let merged =
            merge(vec![f(Severity::Warn, "87% used"), f(Severity::Critical, "full in 3h")]);
        assert_eq!(merged, [f(Severity::Critical, "87% used; full in 3h")]);
        let other = Finding { code: IncidentCode::OomKilled, ..f(Severity::Warn, "oom") };
        assert_eq!(merge(vec![f(Severity::Warn, "a"), other.clone()]).len(), 2);
    }
}
