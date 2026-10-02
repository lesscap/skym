use super::*;
use bollard::models::{EventActor, EventMessage};

fn inspect(name: &str) -> ContainerInspectResponse {
    let path = format!("{}/tests/fixtures/docker/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn labels(pairs: &[(&str, &str)]) -> Labels {
    pairs.iter().map(|(k, v)| (format!("{COMPOSE}{k}"), v.to_string())).collect()
}

fn key(project: &str, service: &str) -> WorkloadKey {
    WorkloadKey { host: "x".into(), project: project.into(), service: service.into() }
}

#[test]
fn workload_rule() {
    use RestartPolicyNameEnum::*;
    let compose = labels(&[("service", "api")]);
    let oneoff = labels(&[("service", "api"), ("oneoff", "True")]);
    let none = Labels::new();
    let cases = [
        (Some(&ALWAYS), &none, true),
        (Some(&UNLESS_STOPPED), &none, true),
        (Some(&ON_FAILURE), &none, true),
        (Some(&NO), &none, false),
        (None, &none, false),
        (Some(&NO), &compose, true),
        (Some(&NO), &oneoff, false),
        (Some(&ALWAYS), &oneoff, true),
    ];
    for (policy, labels, expected) in cases {
        assert_eq!(is_workload(policy, labels), expected, "{policy:?} {labels:?}");
    }
}

#[test]
fn workload_keys() {
    let replica =
        |n: &str| labels(&[("project", "dify"), ("service", "api"), ("container-number", n)]);
    assert_eq!(workload_key("x", "/dify-api-1", &replica("1")), key("dify", "api"));
    assert_eq!(workload_key("x", "/dify-api-2", &replica("2")), key("dify", "api#2"));
    assert_eq!(workload_key("x", "/legacy", &Labels::new()), key("-", "legacy"));
}

#[test]
fn classification_by_repository_name() {
    let cases = [
        ("postgres:16-alpine", WorkloadKind::Datastore, Some(DatastoreKind::Postgres)),
        ("bitnami/postgresql:16.3.0", WorkloadKind::Datastore, Some(DatastoreKind::Postgres)),
        ("registry.example.com:5000/redis:7", WorkloadKind::Datastore, Some(DatastoreKind::Redis)),
        ("mariadb:11", WorkloadKind::Datastore, Some(DatastoreKind::Mysql)),
        ("nginx:latest", WorkloadKind::Proxy, None),
        ("registry.example.com/team/postgres-backup:1", WorkloadKind::App, None),
    ];
    for (image, kind, datastore) in cases {
        assert_eq!(classify(image), (kind, datastore), "{image}");
    }
}

#[test]
fn facts_from_inspect_never_carry_env_or_foreign_labels() {
    let pg = facts_of(&inspect("inspect-compose-postgres.json"));
    assert_eq!((pg.kind, pg.datastore), (WorkloadKind::Datastore, Some(DatastoreKind::Postgres)));
    assert_eq!(pg.restart_policy.as_deref(), Some("unless-stopped"));
    assert_eq!(pg.log_max_size.as_deref(), Some("10m"));
    assert_eq!(pg.ports, ["127.0.0.1:65432->5432/tcp"]);
    assert!(pg.healthcheck);
    assert_eq!(pg.memory_limit_bytes, Some(1_073_741_824));
    assert_eq!(pg.labels.len(), 4, "the compose labels, nothing else");
    assert!(pg.labels.keys().all(|k| k.starts_with(COMPOSE)));
    let json = serde_json::to_string(&pg).unwrap();
    assert!(!json.contains("hunter2") && !json.contains("apr1"), "{json}");

    let worker = facts_of(&inspect("inspect-plain-exited.json"));
    assert_eq!(worker.restart_policy, None, "\"no\" means no policy");
    assert!(!worker.healthcheck, "NONE disables the image's healthcheck");
    assert_eq!(worker.log_max_size, None);
    assert_eq!(worker.memory_limit_bytes, None, "0 means no limit");
}

#[test]
fn state_from_inspect() {
    let pg = state_of(&inspect("inspect-compose-postgres.json"), vec![], Some(1));
    assert_eq!((pg.run, pg.health, pg.exit_code), (RunState::Running, Some(Health::Healthy), None));
    let worker = state_of(&inspect("inspect-plain-exited.json"), vec![], None);
    assert_eq!((worker.run, worker.exit_code), (RunState::Exited, Some(137)));
    assert_eq!(timestamp("0001-01-01T00:00:00Z"), None);
    assert_eq!(timestamp("1970-01-01T00:00:00Z"), None);
    assert_eq!(timestamp("2026-10-01T11:50:00Z").map(|t| t.as_second()), Some(1_790_855_400));
}

#[test]
fn run_state_and_health_mapping() {
    let state = |status: &str, health: Option<&str>| {
        let health = health.map(|h| serde_json::json!({ "Status": h }));
        let i: ContainerInspectResponse = serde_json::from_value(
            serde_json::json!({ "State": { "Status": status, "Health": health } }),
        )
        .unwrap();
        let s = state_of(&i, vec![], None);
        (s.run, s.health)
    };
    let cases = [
        ("running", Some("healthy"), RunState::Running, Some(Health::Healthy)),
        ("restarting", Some("unhealthy"), RunState::Restarting, Some(Health::Unhealthy)),
        ("paused", Some("starting"), RunState::Paused, Some(Health::Starting)),
        ("exited", Some("none"), RunState::Exited, None),
        ("dead", None, RunState::Dead, None),
        ("removing", None, RunState::Dead, None),
        ("created", None, RunState::Created, None),
    ];
    for (status, health, run, expected_health) in cases {
        assert_eq!(state(status, health), (run, expected_health), "{status}");
    }
}

fn event(action: &str, exit: &str, secs: i64) -> ContainerEvent {
    container_event("x", &message(action, exit, secs)).unwrap()
}

fn message(action: &str, exit: &str, secs: i64) -> EventMessage {
    let attrs = HashMap::from([
        ("name".to_string(), "dify-api-1".to_string()),
        ("exitCode".to_string(), exit.to_string()),
        (format!("{COMPOSE}project"), "dify".to_string()),
        (format!("{COMPOSE}service"), "api".to_string()),
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

#[test]
fn working_set_excludes_inactive_cache() {
    assert_eq!(working_set(1000, "anon 600\ninactive_file 300\nactive_file 100\n"), 700);
    assert_eq!(working_set(1000, ""), 1000);
}

#[test]
fn winners_keep_one_container_per_key_whatever_the_order() {
    let container = |id: &str, status: &str, created: &str, labels: serde_json::Value| {
        serde_json::from_value::<ContainerInspectResponse>(serde_json::json!({
            "Id": id, "Name": format!("/{id}"), "Created": created,
            "State": { "Status": status, "Running": status == "running" },
            "Config": { "Labels": labels },
            "HostConfig": { "RestartPolicy": { "Name": "no" } }
        }))
        .unwrap()
    };
    let compose = serde_json::json!({ "com.docker.compose.project": "app", "com.docker.compose.service": "api" });
    let old = container("old", "exited", "2026-10-01T00:00:00Z", compose.clone());
    let new = container("new", "running", "2026-10-02T00:00:00Z", compose.clone());
    let older_running = container("older", "running", "2026-09-01T00:00:00Z", compose);
    let job = container("job", "exited", "2026-10-01T00:00:00Z", serde_json::json!({}));
    for inspects in [
        vec![old.clone(), new.clone(), job.clone()],
        vec![new.clone(), old.clone(), job.clone()],
        vec![older_running.clone(), new.clone()],
        vec![new.clone(), older_running.clone()],
    ] {
        let (chosen, _) = winners("x", &inspects);
        let ids: Vec<&str> = chosen.iter().filter_map(|(_, i)| i.id.as_deref()).collect();
        assert_eq!(ids, ["new"]);
    }
    let (_, transient) = winners("x", &[job]);
    assert_eq!(transient, TransientCounts { running: 0, exited: 1 });
}
