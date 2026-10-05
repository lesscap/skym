//! How values read in the view: sizes, rates, CPU, times, ages and short reasons. Pure.

use crate::problems::Age;
use jiff::Timestamp;
use skym_core::time::format_duration;
use skym_core::view::{EndpointOverview, HostOverview};

/// A time as the clock on the wall shows it.
pub fn local(t: Timestamp, format: &str) -> String {
    t.to_zoned(jiff::tz::TimeZone::system()).strftime(format).to_string()
}

/// `5m`, `3d4h`: how long ago.
pub fn ago(now: Timestamp, then: Timestamp) -> String {
    format_duration(now.duration_since(then))
}

pub fn age(a: Age) -> String {
    match a {
        Age::Exact(d) => format_duration(d),
        Age::AtLeast(d) => format!("≥{}", format_duration(d)),
    }
}

/// An incident's detail without Docker's boilerplate, which would push the cause out of
/// a narrow column.
pub fn reason(detail: &str) -> String {
    detail.replace("OCI runtime exec failed: exec failed: unable to start container process: ", "")
}

pub fn event(kind: &skym_core::model::EventKind) -> String {
    use skym_core::model::EventKind;
    match kind {
        EventKind::Deployed { from, to } => format!("deployed {to} (was {from})"),
        EventKind::ConfigChanged => "configuration changed".into(),
        EventKind::Restarted => "restarted".into(),
        EventKind::OomKilled => "OOM killed".into(),
        EventKind::HostRebooted => "host rebooted".into(),
        EventKind::KernelChanged => "kernel changed".into(),
        EventKind::Unknown => "(an event this version does not know)".into(),
    }
}

/// `80G`, `512M`, `3.2T`.
pub fn size(bytes: u64) -> String {
    let units = [("T", 1e12), ("G", 1e9), ("M", 1e6)];
    let b = bytes as f64;
    match units.iter().find(|(_, scale)| b >= *scale) {
        // One decimal below 10 (as it will be printed), none from there.
        Some((unit, scale)) if (b / scale * 10.0).round() >= 100.0 => {
            format!("{:.0}{unit}", b / scale)
        }
        Some((unit, scale)) => format!("{:.1}{unit}", b / scale),
        None => format!("{}K", bytes / 1000),
    }
}

/// `1.4 / 3.9 GB`.
pub fn memory(used: Option<u64>, total: Option<u64>) -> String {
    let gb = |b: u64| format!("{:.1}", b as f64 / 1e9);
    match (used, total) {
        (Some(used), Some(total)) => format!("{} / {} GB", gb(used), gb(total)),
        (Some(used), None) => format!("{} GB", gb(used)),
        _ => "—".into(),
    }
}

/// `120M / 512M`, or `120M` without a limit.
pub fn usage(used: u64, limit: Option<u64>) -> String {
    limit.map_or_else(|| size(used), |l| format!("{} / {}", size(used), size(l)))
}

/// `1 app`, `43 apps`.
pub fn app_count(n: usize) -> String {
    if n == 1 { "1 app".into() } else { format!("{n} apps") }
}

/// `23% busy · 4% iowait · load 0.93`; steal only from 1%, when a neighbour on the
/// hypervisor is taking time.
pub fn cpu_use(h: &HostOverview) -> String {
    let parts: Vec<String> = [
        h.cpu_percent.map(|p| format!("{p:.0}% busy")),
        h.iowait_percent.map(|p| format!("{p:.0}% iowait")),
        h.steal_percent.filter(|p| *p >= 1.0).map(|p| format!("{p:.0}% steal")),
        Some(h.load_1m.map_or("load —".into(), |l| format!("load {l:.2}"))),
    ]
    .into_iter()
    .flatten()
    .collect();
    parts.join(" · ")
}

/// `4 · 23%`: CPUs and how busy they were since the previous report.
pub fn cpu_cell(h: &HostOverview) -> String {
    let busy = h.cpu_percent.map(|p| format!("{p:.0}%"));
    let parts: Vec<String> = h.cpu_count.map(|n| n.to_string()).into_iter().chain(busy).collect();
    parts.join(" · ")
}

/// How a URL answered: `200 in 84ms` (`with_status`) or `84ms`, else why there is none.
pub fn answer(e: &EndpointOverview, with_status: bool) -> String {
    match (e.http_status, e.latency_ms) {
        (Some(s), Some(ms)) if with_status => format!("{s} in {ms}ms"),
        (Some(_), Some(ms)) => format!("{ms}ms"),
        _ if e.last_probe_ago.is_none() => "not probed yet".into(),
        _ => "no answer".into(),
    }
}

/// CPUs kept busy: `0.30`, `12.5`.
pub fn cores(c: f32) -> String {
    if c < 9.995 { format!("{c:.2}") } else { format!("{c:.1}") }
}

/// `540B/s`, `1.2MB/s`.
pub fn rate(bytes_per_s: u64) -> String {
    match bytes_per_s {
        0..1000 => format!("{bytes_per_s}B/s"),
        b => format!("{}B/s", size(b)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_at_a_glance() {
        assert_eq!(size(80_000_000_000), "80G");
        assert_eq!(size(3_200_000_000_000), "3.2T");
        assert_eq!(size(512_000_000), "512M");
        assert_eq!(size(1_500_000_000), "1.5G");
        assert_eq!(size(9_999_000), "10M", "rounded up to ten, so no decimal");
        assert_eq!(size(9_940_000), "9.9M");
        assert_eq!(size(4_000), "4K");
        assert_eq!(memory(Some(1_400_000_000), Some(3_900_000_000)), "1.4 / 3.9 GB");
        assert_eq!(memory(None, Some(1)), "—");
        assert_eq!(
            (usage(120_000_000, Some(512_000_000)), usage(120_000_000, None)),
            ("120M / 512M".into(), "120M".into())
        );
    }

    #[test]
    fn cores_keep_two_decimals_under_ten() {
        let shown = [0.3, 1.2, 9.99, 9.996, 12.5, 128.0].map(cores);
        assert_eq!(shown, ["0.30", "1.20", "9.99", "10.0", "12.5", "128.0"]);
    }

    #[test]
    fn rates_and_answers() {
        assert_eq!((rate(540), rate(1_200_000)), ("540B/s".into(), "1.2MB/s".into()));
        let probed = |status, ms| EndpointOverview {
            http_status: status,
            latency_ms: ms,
            last_probe_ago: Some("20s".into()),
            ..serde_json::from_value(
                serde_json::json!({ "url": "https://a.example/", "status": "ok" }),
            )
            .unwrap()
        };
        assert_eq!(answer(&probed(Some(200), Some(84)), true), "200 in 84ms");
        assert_eq!(answer(&probed(Some(200), Some(84)), false), "84ms");
        assert_eq!(answer(&probed(None, None), false), "no answer");
        let never = EndpointOverview { last_probe_ago: None, ..probed(None, None) };
        assert_eq!(answer(&never, true), "not probed yet");
    }
}
