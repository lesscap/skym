use super::*;
use bollard::models::EventActor;
use std::collections::HashMap;

fn key(project: &str, service: &str) -> WorkloadKey {
    WorkloadKey { host: "x".into(), project: project.into(), service: service.into() }
}

fn event(action: &str, exit: &str, secs: i64) -> ContainerEvent {
    container_event("x", &message(action, exit, secs)).unwrap()
}

fn message(action: &str, exit: &str, secs: i64) -> EventMessage {
    let attrs = HashMap::from([
        ("name".to_string(), "dify-api-1".to_string()),
        ("exitCode".to_string(), exit.to_string()),
        ("com.docker.compose.project".to_string(), "dify".to_string()),
        ("com.docker.compose.service".to_string(), "api".to_string()),
    ]);
    EventMessage {
        action: Some(action.into()),
        actor: Some(EventActor { id: Some("c1".into()), attributes: Some(attrs) }),
        time: Some(1_790_000_000 + secs),
        ..Default::default()
    }
}

#[test]
fn event_time_prefers_nanoseconds() {
    let m = EventMessage { time_nano: Some(1_790_000_000_500_000_000), ..message("oom", "", 0) };
    let e = container_event("x", &m).unwrap();
    assert_eq!((e.ts.as_second(), e.ts.subsec_millisecond()), (1_790_000_000, 500));
    assert_eq!(e.action, Action::Oom);
    assert!(container_event("x", &message("start", "", 0)).is_none());
}

#[test]
fn crash_restarts_exclude_requested_stops() {
    let events = [
        event("die", "1", 0),   // crash
        event("kill", "", 100), // docker stop …
        event("die", "137", 105),
        event("die", "0", 200), // clean exit
        event("die", "139", 300),
    ];
    let restarts = crash_restarts(&events);
    let times: Vec<i64> = restarts[&key("dify", "api")].iter().map(|t| t.as_second()).collect();
    assert_eq!(times, [1_790_000_000, 1_790_000_300]);
}

#[test]
fn ooms_merge_events_with_inspect_without_duplicates() {
    let since = Timestamp::from_second(1_790_000_000).unwrap();
    let late = Timestamp::from_second(1_790_000_500).unwrap();
    let old = Timestamp::from_second(1_789_000_000).unwrap();
    let events = [event("die", "137", 5), event("oom", "", 10)];
    let exits = [(key("dify", "api"), late), (key("-", "worker"), late), (key("-", "old"), old)];
    let found: Vec<(WorkloadKey, i64)> = ooms(&events, &exits, since)
        .into_iter()
        .filter_map(|e| match e {
            LocalEvent::OomKilled { workload, ts } => Some((workload?, ts.as_second())),
            LocalEvent::Unknown => None,
        })
        .collect();
    let at = |s: i64| 1_790_000_000 + s;
    assert_eq!(found, [(key("dify", "api"), at(10)), (key("-", "worker"), at(500))]);
}
