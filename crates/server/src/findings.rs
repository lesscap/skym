//! Findings only the server can make, from history: lost heartbeats and disk projection.

use jiff::{SignedDuration, Timestamp};
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
    let full_in = SignedDuration::from_secs_f64(total.saturating_sub(last.1) as f64 / slope);
    let (week, day) = (SignedDuration::from_hours(24 * 7), SignedDuration::from_hours(24));
    let matched = if open { full_in <= week } else { full_in < week };
    matched.then(|| (if full_in < day { Severity::Critical } else { Severity::Warn }, full_in))
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
