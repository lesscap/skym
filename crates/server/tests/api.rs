//! The HTTP API end to end, against an in-memory database.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use flate2::{Compression, write::GzEncoder};
use http_body_util::BodyExt;
use jiff::{SignedDuration, Timestamp};
use serde_json::Value;
use skym_core::model::RunState;
use skym_core::report::Report;
use skym_server::api::{AppState, router};
use skym_server::config::{Customer, HostEntry, Mute, Reader, ServerConfig, sha256_hex};
use skym_server::evaluate::heartbeat_once;
use skym_server::{db, store::Store};
use std::io::Write;
use tower::ServiceExt;

const HOST_TOKEN: &str = "host-x-token";
const READER_TOKEN: &str = "reader-token";

fn app() -> (Router, AppState) {
    app_with(vec![])
}

fn app_with(mute: Vec<Mute>) -> (Router, AppState) {
    let cfg = ServerConfig {
        mute,
        customers: vec![Customer { id: "acme".into(), name: "Acme".into() }],
        hosts: vec![HostEntry {
            id: "x".into(),
            customer: "acme".into(),
            token_sha256: sha256_hex(HOST_TOKEN),
        }],
        readers: vec![Reader { name: "ops".into(), token_sha256: sha256_hex(READER_TOKEN) }],
        ..ServerConfig::default()
    };
    let state = AppState::new(Store::new(db::open_in_memory().unwrap()), cfg, Timestamp::now());
    (router(state.clone()), state)
}

/// A healthy report from host x, `mins_ago` minutes old.
fn report(mins_ago: i64) -> Report {
    let mut r: Report = skym_core::fixtures::full_report();
    r.ts = Timestamp::now() - SignedDuration::from_mins(mins_ago);
    for g in &mut r.exceptions {
        (g.first_seen, g.last_seen) = (r.ts, r.ts);
    }
    r
}

async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Vec<u8>>,
) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    let gzip = body.as_ref().is_some_and(|b| b.starts_with(&[0x1f, 0x8b]));
    if gzip {
        req = req.header("content-encoding", "gzip");
    }
    let res = app
        .clone()
        .oneshot(req.body(body.map_or_else(Body::empty, Body::from)).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn post(app: &Router, r: &Report) -> StatusCode {
    call(app, "POST", "/api/report", Some(HOST_TOKEN), Some(serde_json::to_vec(r).unwrap())).await.0
}

async fn get(app: &Router, uri: &str) -> Value {
    let (status, body) = call(app, "GET", uri, Some(READER_TOKEN), None).await;
    assert_eq!(status, StatusCode::OK, "{uri}: {body}");
    body
}

#[tokio::test]
async fn tokens_only_open_their_own_door() {
    let (app, _) = app();
    let body = serde_json::to_vec(&report(5)).unwrap();
    assert_eq!(call(&app, "GET", "/api/overview", None, None).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(
        call(&app, "GET", "/api/overview", Some(HOST_TOKEN), None).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&app, "POST", "/api/report", Some(READER_TOKEN), Some(body.clone())).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&app, "POST", "/api/report", None, Some(body.clone())).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&app, "POST", "/api/report", Some(HOST_TOKEN), Some(body)).await.0,
        StatusCode::OK
    );
    assert_eq!(call(&app, "GET", "/healthz", None, None).await.0, StatusCode::OK);
}

#[tokio::test]
async fn the_token_decides_which_host_a_report_belongs_to() {
    let (app, _) = app();
    let mut r = report(5);
    r.host = "evil".into();
    r.workloads.iter_mut().for_each(|w| w.key.host = "evil".into());
    assert_eq!(post(&app, &r).await, StatusCode::OK);
    let host = get(&app, "/api/hosts/x").await;
    assert_eq!(host["workloads"].as_array().unwrap().len(), 3);
    assert!(host["workloads"].as_array().unwrap().iter().all(|w| w["key"]["host"] == "x"));
    assert_eq!(
        call(&app, "GET", "/api/hosts/evil", Some(READER_TOKEN), None).await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn malformed_future_and_duplicate_reports() {
    let (app, _) = app();
    let mut bad = report(5);
    bad.workloads[0].key.project = String::new();
    assert_eq!(post(&app, &bad).await, StatusCode::BAD_REQUEST);
    assert_eq!(post(&app, &report(-20)).await, StatusCode::BAD_REQUEST, "20 minutes ahead");
    let (status, _) =
        call(&app, "POST", "/api/report", Some(HOST_TOKEN), Some(b"{not json".to_vec())).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let r = report(5);
    assert_eq!(post(&app, &r).await, StatusCode::OK);
    assert_eq!(post(&app, &r).await, StatusCode::OK, "a replayed report is acknowledged");
    let ex = get(&app, "/api/exceptions?host=x&since=1h&class=business").await;
    assert_eq!(ex["exceptions"][0]["count"], 31, "but not counted twice");
}

#[tokio::test]
async fn gzip_bodies_are_accepted() {
    let (app, _) = app();
    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    gz.write_all(&serde_json::to_vec(&report(5)).unwrap()).unwrap();
    let (status, _) =
        call(&app, "POST", "/api/report", Some(HOST_TOKEN), Some(gz.finish().unwrap())).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(get(&app, "/api/hosts/x").await["status"], "ok");
}

#[tokio::test]
async fn an_incident_from_reports_to_overview_workload_and_timeline() {
    let (app, _) = app();
    for mins in [9, 8] {
        assert_eq!(post(&app, &down(report(mins), 0)).await, StatusCode::OK);
    }
    let overview = get(&app, "/api/overview").await;
    let incident = &overview["customers"][0]["hosts"][0]["incidents"][0];
    assert_eq!(
        (incident["code"].as_str(), incident["severity"].as_str()),
        (Some("WORKLOAD_DOWN"), Some("critical"))
    );
    assert_eq!(overview["status"], "critical");

    let workload = get(&app, incident["links"]["workload"].as_str().unwrap()).await;
    assert_eq!(workload["status"], "critical");

    let mut redeployed = report(7);
    redeployed.workloads[0].facts.as_mut().unwrap().image =
        "registry.example.com/captain:1.4.3".into();
    assert_eq!(post(&app, &redeployed).await, StatusCode::OK);
    let timeline = get(&app, incident["links"]["timeline"].as_str().unwrap()).await;
    let types: Vec<&str> =
        timeline["entries"].as_array().unwrap().iter().filter_map(|e| e["type"].as_str()).collect();
    assert!(types.contains(&"incident_opened") && types.contains(&"event"), "{timeline}");
    let deployed =
        timeline["entries"].as_array().unwrap().iter().find(|e| e["type"] == "event").unwrap();
    assert_eq!(deployed["event"]["type"], "deployed");
}

#[tokio::test]
async fn a_failed_source_does_not_resolve_what_it_could_not_see() {
    let (app, _) = app();
    for mins in [9, 8] {
        post(&app, &down(report(mins), 0)).await;
    }
    let mut blind = report(7);
    blind.workloads.clear();
    blind.errors = vec!["docker: permission denied".into()];
    for mins in [7, 6] {
        blind.ts = Timestamp::now() - SignedDuration::from_mins(mins);
        assert_eq!(post(&app, &blind).await, StatusCode::OK);
    }
    let open = get(&app, "/api/incidents?host=x").await;
    assert_eq!(open["incidents"][0]["code"], "WORKLOAD_DOWN", "still open: {open}");
    assert_eq!(get(&app, "/api/hosts/x").await["errors"][0], "docker: permission denied");
}

#[tokio::test]
async fn a_silent_host_loses_its_heartbeat_and_a_report_restores_it() {
    let (app, state) = app();
    post(&app, &report(1)).await;
    let beat = |now| {
        let state = state.clone();
        async move {
            let started = state.started - SignedDuration::from_hours(1);
            state
                .store
                .call(move |c| {
                    heartbeat_once(c, &["x".into()], started, SignedDuration::from_secs(60), now)
                })
                .await
                .unwrap()
        }
    };
    beat(Timestamp::now() + SignedDuration::from_mins(4)).await;
    let open = get(&app, "/api/incidents").await;
    assert_eq!(open["incidents"][0]["code"], "HEARTBEAT_LOST");
    post(&app, &report(0)).await;
    beat(Timestamp::now()).await;
    assert!(get(&app, "/api/incidents").await["incidents"].as_array().unwrap().is_empty());
    let resolved = get(&app, "/api/incidents?status=resolved").await;
    assert_eq!(resolved["incidents"][0]["code"], "HEARTBEAT_LOST");
}

/// Workload `i` of the report exited with an error.
fn down(mut r: Report, i: usize) -> Report {
    r.workloads[i].state.run = RunState::Exited;
    r.workloads[i].state.exit_code = Some(1);
    r
}

#[tokio::test]
async fn views_scope_incidents_and_ignore_muted_ones() {
    let mute = Mute {
        subject: "workload:x/pg/main".parse().unwrap(),
        code: skym_core::rules::IncidentCode::WorkloadDown,
        reason: Some("being migrated".into()),
        until: None,
    };
    let (app, _) = app_with(vec![mute]);
    for mins in [9, 8] {
        let mut r = down(down(report(mins), 0), 1);
        r.exceptions[0].workload = r.workloads[0].key.clone();
        post(&app, &r).await;
    }
    let host = get(&app, "/api/hosts/x").await;
    let statuses: Vec<(&str, &str)> = host["workloads"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| (w["key"]["service"].as_str().unwrap(), w["status"].as_str().unwrap()))
        .collect();
    assert_eq!(
        statuses,
        [("xray", "ok"), ("api", "critical"), ("main", "ok")],
        "muted pg does not count"
    );
    assert_eq!(host["incidents"].as_array().unwrap().len(), 2, "but is listed");

    assert_eq!(host["customer"], "acme");
    let api = get(&app, "/api/hosts/x/workloads/captain/api").await;
    assert_eq!(api["incidents"].as_array().unwrap().len(), 1, "only its own incident");
    assert_eq!(api["incidents"][0]["subject"], "workload:x/captain/api");
    assert_eq!(api["facts"]["image"], "registry.example.com/captain:1.4.2", "its own facts");
    assert_eq!(api["exceptions"][0]["code"], "SOURCE_MISSING", "this hour's exceptions");
    let pg = get(&app, "/api/hosts/x/workloads/pg/main").await;
    assert_eq!(
        (pg["status"].as_str(), pg["incidents"][0]["muted"].as_bool()),
        (Some("ok"), Some(true))
    );

    let muted = get(&app, "/api/incidents?include_muted=true").await;
    assert_eq!(muted["incidents"].as_array().unwrap().len(), 2);
    assert_eq!(get(&app, "/api/incidents").await["incidents"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn filters_limits_and_truncation() {
    let (app, _) = app();
    for mins in [9, 8] {
        post(&app, &down(down(report(mins), 0), 1)).await;
    }
    let mut redeployed = report(7);
    redeployed.workloads[0].facts.as_mut().unwrap().image = "x:2".into();
    post(&app, &redeployed).await;

    let one = get(&app, "/api/incidents?limit=1").await;
    assert_eq!(
        (one["incidents"].as_array().unwrap().len(), one["truncated"].as_bool()),
        (1, Some(true))
    );
    let both = get(&app, "/api/incidents?limit=2").await;
    assert_eq!(both["truncated"], false);
    let none = get(&app, "/api/incidents?code=OOM_KILLED").await;
    assert!(none["incidents"].as_array().unwrap().is_empty());
    let down = get(&app, "/api/incidents?code=WORKLOAD_DOWN").await;
    assert_eq!(down["incidents"].as_array().unwrap().len(), 2);
    let unknown =
        call(&app, "GET", "/api/exceptions?host=x&class=unknown", Some(READER_TOKEN), None).await;
    assert_eq!(unknown.0, StatusCode::BAD_REQUEST, "unknown names no class to filter by");
    let bad = call(&app, "GET", "/api/incidents?code=NOPE", Some(READER_TOKEN), None).await;
    assert_eq!(bad.0, StatusCode::BAD_REQUEST);
    let timeline = get(&app, "/api/timeline?host=x&limit=1").await;
    assert_eq!(
        (timeline["entries"].as_array().unwrap().len(), timeline["truncated"].as_bool()),
        (1, Some(true))
    );
    let workload_events = get(&app, "/api/hosts/x/workloads/captain/api").await;
    assert_eq!(workload_events["events"][0]["kind"]["type"], "deployed", "events of the last day");
    let zero = get(&app, "/api/incidents?limit=0").await;
    assert_eq!(zero["incidents"].as_array().unwrap().len(), 1, "limit is at least 1");
    let future = (Timestamp::now() + SignedDuration::from_hours(1)).to_string();
    let later = get(&app, &format!("/api/incidents?status=resolved&since={future}")).await;
    assert!(later["incidents"].as_array().unwrap().is_empty());
    assert_eq!(
        call(&app, "GET", "/api/incidents?status=sometimes", Some(READER_TOKEN), None).await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(&app, "GET", "/api/timeline", Some(READER_TOKEN), None).await.0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn a_host_with_only_muted_trouble_is_ok() {
    let mute = Mute {
        subject: "workload:x/pg/main".parse().unwrap(),
        code: skym_core::rules::IncidentCode::WorkloadDown,
        reason: None,
        until: None,
    };
    let (app, _) = app_with(vec![mute]);
    for mins in [9, 8] {
        post(&app, &down(report(mins), 1)).await;
    }
    assert_eq!(get(&app, "/api/hosts/x").await["status"], "ok");
}

#[tokio::test]
async fn resolved_incidents_respect_since_and_timelines_their_limit() {
    let (app, _) = app();
    for mins in [9, 8] {
        post(&app, &down(report(mins), 1)).await;
    }
    for mins in [7, 6] {
        post(&app, &report(mins)).await;
    }
    let resolved = get(&app, "/api/incidents?status=resolved").await;
    assert_eq!(resolved["incidents"].as_array().unwrap().len(), 1);
    let future = (Timestamp::now() + SignedDuration::from_hours(1)).to_string();
    let none = get(&app, &format!("/api/incidents?status=resolved&since={future}")).await;
    assert!(none["incidents"].as_array().unwrap().is_empty());
    let pg = get(&app, "/api/timeline?host=x&workload=pg/main&limit=1").await;
    assert_eq!(
        (pg["entries"].as_array().unwrap().len(), pg["truncated"].as_bool()),
        (1, Some(true)),
        "{pg}"
    );
    let both = get(&app, "/api/timeline?host=x&workload=pg/main&limit=2").await;
    assert_eq!(both["truncated"], false);
}

#[tokio::test]
async fn a_timeline_of_events_alone_is_truncated_too() {
    let (app, _) = app();
    post(&app, &report(9)).await;
    for (mins, tag) in [(8, "2"), (7, "3")] {
        let mut r = report(mins);
        r.workloads[0].facts.as_mut().unwrap().image =
            format!("registry.example.com/captain:{tag}");
        post(&app, &r).await;
    }
    let one = get(&app, "/api/timeline?host=x&workload=captain/api&limit=1").await;
    assert_eq!(
        (one["entries"].as_array().unwrap().len(), one["truncated"].as_bool()),
        (1, Some(true)),
        "{one}"
    );
}

#[tokio::test]
async fn reports_without_a_token_are_refused_before_their_body_is_read() {
    let (app, _) = app();
    let mut gz = GzEncoder::new(Vec::new(), Compression::best());
    gz.write_all(&vec![b' '; 40 << 20]).unwrap(); // 40 MiB of spaces, a few KiB compressed
    let bomb = gz.finish().unwrap();
    assert!(bomb.len() < 1 << 20);
    assert_eq!(
        call(&app, "POST", "/api/report", None, Some(bomb.clone())).await.0,
        StatusCode::UNAUTHORIZED
    );
    let (status, _) = call(&app, "POST", "/api/report", Some(HOST_TOKEN), Some(bomb)).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "authenticated, then bounded");
}

#[tokio::test]
async fn reports_from_a_clock_slightly_ahead_are_visible_at_once() {
    let (app, _) = app();
    assert_eq!(post(&app, &report(-5)).await, StatusCode::OK);
    let ex = get(&app, "/api/exceptions?host=x").await;
    assert_eq!(ex["exceptions"].as_array().unwrap().len(), 1);
}
