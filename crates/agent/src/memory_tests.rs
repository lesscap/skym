use super::*;
use crate::collect::Workload;
use skym_core::model::HostState;

fn t(sec: i64) -> Timestamp {
    Timestamp::from_second(1_790_000_000 + sec).unwrap()
}

fn workload(service: &str, restarts: &[i64]) -> Workload {
    let w = skym_core::fixtures::full_report().workloads.remove(0);
    let mut state = w.state;
    state.restarts = restarts.iter().map(|s| t(*s)).collect();
    let key = WorkloadKey { service: service.into(), ..w.key };
    Workload { key, facts: w.facts.unwrap(), state }
}

fn pass(workloads: Vec<Workload>) -> Collected {
    Collected { workloads, events_read: true, workloads_complete: true, ..Default::default() }
}

fn restarts(c: &Collected) -> Vec<i64> {
    c.workloads[0].state.restarts.iter().map(|ts| ts.as_second() - 1_790_000_000).collect()
}

#[test]
fn restarts_are_judged_once_and_remembered_for_an_hour() {
    let mut m = Memory::default();
    assert_eq!(m.events_since(t(100)), t(100) - HOUR, "first pass looks back an hour");
    let mut first = pass(vec![workload("api", &[10, 50])]);
    m.remember(&mut first, t(100));
    assert_eq!(restarts(&first), [10, 50]);
    // the next query reaches 15 s back for context; restarts up to the previous end are old
    assert_eq!(m.events_since(t(160)), t(85));
    let mut second = pass(vec![workload("api", &[95, 100, 130])]);
    m.remember(&mut second, t(160));
    assert_eq!(restarts(&second), [10, 50, 130]);
    let mut later = pass(vec![workload("api", &[])]);
    m.remember(&mut later, t(3640));
    assert_eq!(restarts(&later), [50, 130], "an hour later the first is gone");
}

#[test]
fn a_failed_events_query_is_retried_from_where_the_last_good_one_ended() {
    let mut m = Memory::default();
    m.remember(&mut pass(vec![workload("api", &[])]), t(100));
    let mut failed = Collected { events_read: false, ..pass(vec![workload("api", &[])]) };
    m.remember(&mut failed, t(160));
    assert_eq!(m.events_since(t(220)), t(85), "still from the pass at 100");
    let mut caught = pass(vec![workload("api", &[130])]);
    m.remember(&mut caught, t(220));
    assert_eq!(restarts(&caught), [130], "the crash during the failure counts");
    assert_eq!(m.events_since(t(9000)), t(9000) - HOUR, "never more than an hour back");
}

#[test]
fn a_workload_that_disappears_is_forgotten_but_not_from_an_incomplete_list() {
    let mut m = Memory::default();
    m.remember(&mut pass(vec![workload("api", &[10])]), t(100));
    let mut failed = Collected { workloads_complete: false, ..pass(vec![]) };
    m.remember(&mut failed, t(130));
    let mut kept = pass(vec![workload("api", &[])]);
    m.remember(&mut kept, t(140));
    assert_eq!(restarts(&kept), [10]);
    m.remember(&mut pass(vec![]), t(160));
    let mut back = pass(vec![workload("api", &[])]);
    m.remember(&mut back, t(220));
    assert!(restarts(&back).is_empty());
}

#[test]
fn host_oom_kills_are_those_no_container_accounts_for() {
    let mut m = Memory::default();
    let counted = |count: u64, container_ooms: &[i64]| Collected {
        oom_kill_count: Some(count),
        local_events: container_ooms
            .iter()
            .map(|s| LocalEvent::OomKilled { ts: t(*s), workload: Some(workload("api", &[]).key) })
            .collect(),
        ..Default::default()
    };
    let host_events = |c: &Collected| {
        c.local_events
            .iter()
            .filter(|e| matches!(e, LocalEvent::OomKilled { workload: None, .. }))
            .count()
    };
    let mut first = counted(7, &[]);
    m.remember(&mut first, t(100));
    assert_eq!(host_events(&first), 0, "the first reading is a baseline");
    let mut covered = counted(8, &[90, 130]); // the kill at 90 was counted before
    m.remember(&mut covered, t(160));
    assert_eq!(host_events(&covered), 0);
    let mut unexplained = counted(10, &[200]);
    m.remember(&mut unexplained, t(220));
    assert_eq!(host_events(&unexplained), 1);
    let mut unreadable = Collected { oom_kill_count: None, ..counted(0, &[]) };
    m.remember(&mut unreadable, t(280));
    let mut again = counted(10, &[]);
    m.remember(&mut again, t(340));
    assert_eq!(host_events(&again), 0, "an unreadable counter keeps the last reading");
}

#[test]
fn systemd_restarts_are_counted_from_the_second_reading() {
    let mut m = Memory::default();
    let unit = |count: u32| {
        let w = workload("xray", &[]);
        Collected {
            unit_restart_counts: BTreeMap::from([(w.key.clone(), count)]),
            workloads: vec![w],
            ..Default::default()
        }
    };
    let mut first = unit(4);
    m.remember(&mut first, t(100));
    assert!(restarts(&first).is_empty(), "restarts before skym started are unknown");
    let mut second = unit(7);
    m.remember(&mut second, t(160));
    let times = &second.workloads[0].state.restarts;
    assert_eq!(times.len(), 3, "three distinct times");
    let secs: Vec<i64> = times.iter().map(|ts| ts.as_second() - 1_790_000_000).collect();
    assert_eq!(secs, [158, 159, 160], "whole seconds, so the server keeps three events");
    let mut reset = unit(1);
    m.remember(&mut reset, t(220));
    assert_eq!(reset.workloads[0].state.restarts.len(), 3, "a reset counter adds none");
}

/// A pass with the host's state and these counters: CPU busy and idle jiffies and eth0's
/// received bytes, read at `at` centiseconds of uptime.
fn reading(at: u64, cpu: Option<(u64, u64)>, rx: Option<u64>) -> Collected {
    use crate::collect::host::CpuTimes;
    let report = skym_core::fixtures::full_report();
    let state = HostState { cpu_percent: None, net_rx_bytes_per_s: None, ..report.host_state };
    let counters = Counters {
        cpu: cpu.map(|(busy, idle)| (at, CpuTimes { busy, idle, iowait: 0, steal: 0 })),
        net: rx.map(|rx| (at, [("eth0".to_string(), (rx, 0))].into())),
        ..Counters::default()
    };
    Collected { host: Some((report.host_facts.unwrap(), state)), counters, ..pass(vec![]) }
}

fn rates(c: &Collected) -> (Option<f32>, Option<u64>) {
    let state = &c.host.as_ref().unwrap().1;
    (state.cpu_percent, state.net_rx_bytes_per_s)
}

#[test]
fn rates_come_from_the_previous_reading() {
    let mut m = Memory::default();
    let mut first = reading(10_000, Some((100, 100)), Some(1_000));
    m.remember(&mut first, t(0));
    assert_eq!(rates(&first), (None, None), "nothing to compare with yet");
    let mut second = reading(16_000, Some((150, 150)), Some(61_000));
    m.remember(&mut second, t(60));
    assert_eq!(rates(&second), (Some(50.0), Some(1_000)));

    // A pass without host state still keeps its readings; one that failed a reading keeps
    // the previous, so the next rate spans both intervals.
    let mut hostless = Collected { host: None, ..reading(22_000, Some((250, 250)), None) };
    m.remember(&mut hostless, t(120));
    let mut third = reading(28_000, Some((250, 350)), Some(181_000));
    m.remember(&mut third, t(180));
    assert_eq!(rates(&third), (Some(0.0), Some(1_000)), "net over 120 s since 16_000");

    let mut rebooted = reading(500, Some((10, 10)), Some(10));
    m.remember(&mut rebooted, t(240));
    assert_eq!(rates(&rebooted), (None, None), "uptime went back: a reboot");
    let mut after = reading(6_500, Some((40, 40)), Some(60_010));
    m.remember(&mut after, t(300));
    assert_eq!(rates(&after), (Some(50.0), Some(1_000)), "from the reboot's reading on");
}

#[test]
fn a_one_off_pass_measures_from_given_counters() {
    let before = reading(10_000, Some((100, 100)), Some(0)).counters;
    let mut pass = reading(10_100, Some((110, 110)), Some(500));
    Memory::starting_from(before).rates(&mut pass);
    assert_eq!(rates(&pass), (Some(50.0), Some(500)));
}

/// A pass with these workloads, each read at uptime `at` with `(service, instance, ns)`.
fn cpu_pass(at: u64, services: &[&str], samples: &[(&str, &str, u64)]) -> Collected {
    use crate::collect::host::CpuTimes;
    let mut c = pass(services.iter().map(|s| workload(s, &[])).collect());
    (c.counters.at, c.counters.cpu) = (Some(at), Some((at, CpuTimes::default())));
    c.counters.workloads = samples
        .iter()
        .map(|(service, instance, ns)| {
            let s = Sample { at, instance: instance.to_string(), ns: *ns };
            (workload(service, &[]).key, s)
        })
        .collect();
    c
}

fn cores_of(c: &Collected) -> Vec<Option<f32>> {
    c.workloads.iter().map(|w| w.state.cpu_cores).collect()
}

#[test]
fn a_workloads_cpu_comes_from_two_readings_of_the_same_run() {
    let mut m = Memory::default();
    let mut first = cpu_pass(10_000, &["api"], &[("api", "a", 0)]);
    m.rates(&mut first);
    assert_eq!(cores_of(&first), [None], "nothing to compare with yet");
    let mut second = cpu_pass(16_000, &["api"], &[("api", "a", 30_000_000_000)]);
    m.rates(&mut second);
    assert_eq!(cores_of(&second), [Some(0.5)], "30 s of CPU in 60 s");
    let mut restarted = cpu_pass(22_000, &["api"], &[("api", "b", 1_000_000_000)]);
    m.rates(&mut restarted);
    assert_eq!(cores_of(&restarted), [None], "a new run counts from zero");
    let mut after = cpu_pass(28_000, &["api"], &[("api", "b", 7_000_000_000)]);
    m.rates(&mut after);
    assert_eq!(cores_of(&after), [Some(0.1)]);
}

#[test]
fn a_reading_missed_is_kept_while_it_can_still_make_a_rate() {
    let mut m = Memory::default();
    m.rates(&mut cpu_pass(10_000, &["api"], &[("api", "a", 0)]));
    let mut missed = cpu_pass(16_000, &["api"], &[]);
    m.rates(&mut missed);
    assert_eq!(cores_of(&missed), [None]);
    let mut again = cpu_pass(20_000, &["api"], &[("api", "a", 10_000_000_000)]);
    m.rates(&mut again);
    assert_eq!(cores_of(&again), [Some(0.1)], "over 100 s, from the reading kept");
    m.rates(&mut cpu_pass(80_001, &["api"], &[]));
    let mut late = cpu_pass(81_000, &["api"], &[("api", "a", 20_000_000_000)]);
    m.rates(&mut late);
    assert_eq!(cores_of(&late), [None], "a reading over ten minutes old is gone");
}

#[test]
fn a_running_workload_without_a_counter_is_reported_once() {
    let mut m = Memory::default();
    let c = pass(vec![workload("api", &[]), workload("web", &[])]);
    assert!(c.workloads.iter().all(|w| w.state.run == skym_core::model::RunState::Running));
    let keys = |ks: Vec<WorkloadKey>| ks.into_iter().map(|k| k.service).collect::<Vec<_>>();
    assert_eq!(keys(m.uncounted(&c)), ["api", "web"]);
    assert_eq!(keys(m.uncounted(&c)), Vec::<String>::new(), "said once");
    let cut_short = Collected { workloads_complete: false, ..pass(vec![workload("web", &[])]) };
    m.uncounted(&cut_short);
    assert!(m.uncounted(&c).is_empty(), "a listing cut short forgets nothing");
    m.uncounted(&pass(vec![workload("web", &[])]));
    assert_eq!(keys(m.uncounted(&c)), ["api"], "gone from a complete one: said again");
    let counted = crate::collect::host::Target {
        key: workload("db", &[]).key,
        instance: "x".into(),
        cpu: crate::collect::cgroup::CpuFile::V2("/nonexistent".into()),
    };
    let c = Collected { targets: vec![counted], ..pass(vec![workload("db", &[])]) };
    assert!(m.uncounted(&c).is_empty(), "one with a counter is not reported");
}
