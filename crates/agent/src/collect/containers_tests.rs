use super::*;

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
        assert_eq!(classify(image, &[], &[]), (kind, datastore), "{image}");
    }
}

#[test]
fn a_datastore_image_running_a_job_is_an_application() {
    let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let entry = args(&["docker-entrypoint.sh"]);
    let servers = [
        ("postgres:17", entry.clone(), args(&["postgres", "-c", "max_connections=200"])),
        ("redis:7", entry.clone(), args(&["--appendonly", "yes"])),
        (
            "bitnami/postgresql:16",
            args(&["/opt/bitnami/scripts/postgresql/entrypoint.sh"]),
            args(&["/opt/bitnami/scripts/postgresql/run.sh"]),
        ),
        ("mariadb:11", vec![], vec![]),
    ];
    for (image, entrypoint, cmd) in &servers {
        assert_eq!(classify(image, entrypoint, cmd).0, WorkloadKind::Datastore, "{image} {cmd:?}");
    }
    let jobs = [
        (entry.clone(), args(&["/bin/sh", "-ec", "while true; do pg_dump --host=database; done"])),
        (args(&["/bin/sh", "-c", "pg_dump --host=database"]), vec![]),
        (entry, args(&["pg_dump", "--host=database"])),
    ];
    for (entrypoint, cmd) in &jobs {
        assert_eq!(
            classify("postgres:17-alpine", entrypoint, cmd),
            (WorkloadKind::App, None),
            "{cmd:?}"
        );
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
fn state_says_since_when_and_why() {
    let state = |json: serde_json::Value| {
        state_of(&serde_json::from_value::<ContainerInspectResponse>(json).unwrap(), vec![], None)
    };
    let exited = state(serde_json::json!({ "State": {
        "Status": "exited", "ExitCode": 137, "OOMKilled": true,
        "StartedAt": "2025-10-01T00:00:00Z", "FinishedAt": "2025-10-07T22:42:44Z" } }));
    assert_eq!(exited.state_since, timestamp("2025-10-07T22:42:44Z"));
    assert!(exited.oom_killed);
    let long = format!(
        "curl: (7) Failed to connect to 127.0.0.1 port 80; Bearer abc.def {}",
        "x".repeat(300)
    );
    let unhealthy = state(serde_json::json!({ "State": {
        "Status": "running", "StartedAt": "2026-10-01T00:00:00Z", "FinishedAt": "0001-01-01T00:00:00Z",
        "Health": { "Status": "unhealthy", "FailingStreak": 3471,
                    "Log": [{ "Output": "old" }, { "Output": format!("{long}\n") }] } } }));
    assert_eq!(unhealthy.state_since, timestamp("2026-10-01T00:00:00Z"));
    assert_eq!(unhealthy.health_failing_streak, Some(3471));
    let output = unhealthy.health_output.unwrap();
    assert!(output.starts_with("curl: (7)") && output.contains("Bearer [REDACTED]"), "{output}");
    assert_eq!(output.chars().count(), 200);
    let healthy = state(serde_json::json!({ "State": {
        "Status": "running", "Health": { "Status": "healthy", "FailingStreak": 0, "Log": [{ "Output": "ok" }] } } }));
    assert_eq!((healthy.health_failing_streak, healthy.health_output), (None, None));
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
