use super::*;
use skym_core::model::RunState;

fn report() -> Report {
    skym_core::fixtures::full_report()
}

fn stored_by(r: &Report, agent: &str) -> BTreeMap<WorkloadKey, Stored> {
    r.workloads
        .iter()
        .map(|w| {
            let facts = serde_json::to_value(w.facts.as_ref().unwrap()).unwrap();
            (w.key.clone(), Stored { facts, agent: Some(agent.into()) })
        })
        .collect()
}

fn stored(r: &Report) -> BTreeMap<WorkloadKey, Stored> {
    stored_by(r, "0.1.0")
}

fn kinds(events: &[Event]) -> Vec<&EventKind> {
    events.iter().map(|e| &e.kind).collect()
}

#[test]
fn unchanged_facts_make_no_events() {
    let r = report();
    assert!(events(r.host_facts.as_ref(), &stored(&r), &r).is_empty());
}

#[test]
fn image_changes_are_deployments_and_other_changes_config() {
    let old = report();
    let mut new = old.clone();
    new.workloads[0].facts.as_mut().unwrap().image = "registry.example.com/captain:1.4.3".into();
    new.workloads[1].facts.as_mut().unwrap().image_digest =
        Some("sha256:bbbbbbbbbbbbbbbbbbbb".into());
    new.workloads[2].facts.as_mut().unwrap().restart_policy = Some("always".into());
    let found = events(old.host_facts.as_ref(), &stored(&old), &new);
    assert_eq!(
        kinds(&found),
        [
            &EventKind::Deployed {
                from: "registry.example.com/captain:1.4.2".into(),
                to: "registry.example.com/captain:1.4.3".into()
            },
            &EventKind::Deployed { from: String::new(), to: "bbbbbbbbbbbb".into() },
            &EventKind::ConfigChanged,
        ]
    );
    assert_eq!(found[0].subject, Subject::Workload(new.workloads[0].key.clone()));
}

#[test]
fn fields_one_side_lacks_and_facts_from_another_agent_are_not_changes() {
    let r = report();
    let mut older = stored(&r);
    for stored in older.values_mut() {
        stored.facts.as_object_mut().unwrap().remove("log_max_size");
        stored.facts.as_object_mut().unwrap().insert("retired_field".into(), Value::from(1));
    }
    assert!(events(r.host_facts.as_ref(), &older, &r).is_empty());

    let mut upgraded = r.clone();
    upgraded.host_facts.as_mut().unwrap().agent_version = "0.2.0".into();
    upgraded.workloads[2].facts.as_mut().unwrap().restart_policy = Some("always".into());
    assert!(
        events(r.host_facts.as_ref(), &stored(&r), &upgraded).is_empty(),
        "facts from 0.1.0, report from 0.2.0"
    );
    let later =
        Report { host_facts: None, agent_version: Some("0.2.0".into()), ..upgraded.clone() };
    let prev = upgraded.host_facts.as_ref();
    assert!(
        events(prev, &stored(&r), &later).is_empty(),
        "still 0.1.0 facts, even without host facts"
    );
    assert_eq!(
        kinds(&events(prev, &stored_by(&r, "0.2.0"), &later)),
        [&EventKind::ConfigChanged],
        "same agent again"
    );
    upgraded.workloads[0].facts.as_mut().unwrap().image = "x:2".into();
    assert_eq!(
        events(r.host_facts.as_ref(), &stored(&r), &upgraded).len(),
        1,
        "a deployment still counts"
    );
}

#[test]
fn reboots_kernels_restarts_and_oom_kills() {
    let old = report();
    let mut new = old.clone();
    let facts = new.host_facts.as_mut().unwrap();
    facts.boot_time = "2026-10-01T11:00:00Z".parse().unwrap();
    facts.kernel = "6.9.0".into();
    let restart = "2026-10-01T11:58:00Z".parse().unwrap();
    new.workloads[0].state.restarts = vec![restart];
    new.workloads[0].state.run = RunState::Running;
    let key = new.workloads[0].key.clone();
    new.local_events = vec![
        LocalEvent::OomKilled { ts: restart, workload: Some(key.clone()) },
        LocalEvent::OomKilled { ts: restart, workload: None },
        LocalEvent::Unknown,
    ];
    let found = events(old.host_facts.as_ref(), &stored(&old), &new);
    let summary: Vec<(String, &EventKind)> =
        found.iter().map(|e| (e.subject.to_string(), &e.kind)).collect();
    assert_eq!(
        summary,
        [
            ("host:x".to_string(), &EventKind::HostRebooted),
            ("host:x".to_string(), &EventKind::KernelChanged),
            ("workload:x/captain/api".to_string(), &EventKind::Restarted),
            ("workload:x/captain/api".to_string(), &EventKind::OomKilled),
            ("host:x".to_string(), &EventKind::OomKilled),
        ]
    );
    assert_eq!(found[2].ts, restart, "restarts keep their own time");
    assert!(events(None, &BTreeMap::new(), &report()).is_empty(), "nothing stored yet: no diffs");
}

#[test]
fn the_reporting_agent_is_named_by_the_report_itself() {
    let r = report();
    assert_eq!(reporting_agent(&r), Some("0.1.0"), "from host facts when the field is absent");
    let named = Report { host_facts: None, agent_version: Some("0.3.0".into()), ..r.clone() };
    assert_eq!(reporting_agent(&named), Some("0.3.0"));
    let both = Report { agent_version: Some("0.3.0".into()), ..r.clone() };
    assert_eq!(reporting_agent(&both), Some("0.3.0"), "the field wins");
    assert_eq!(reporting_agent(&Report { host_facts: None, ..r }), None);
}
