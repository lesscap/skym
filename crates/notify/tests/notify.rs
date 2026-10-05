//! Passes against local fakes of skym's API, a Feishu bot and healthchecks.io.

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use jiff::{SignedDuration, Timestamp};
use serde_json::{Value, json};
use skym_core::fixtures::incident;
use skym_core::rules::{IncidentCode, Severity};
use skym_core::view::{IncidentList, IncidentView};
use skym_notify::{Notifier, Ping, config::Config};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Fake {
    incidents: Vec<IncidentView>,
    feishu_code: i64,
    posts: Vec<Value>,
    pings: Vec<String>,
}

type Shared = Arc<Mutex<Fake>>;

async fn serve(fake: Shared) -> String {
    let app = Router::new()
        .route(
            "/api/incidents",
            get(|State(f): State<Shared>| async move {
                Json(IncidentList {
                    incidents: f.lock().unwrap().incidents.clone(),
                    truncated: false,
                })
            }),
        )
        .route(
            "/open-apis/bot/v2/hook/{token}",
            post(|State(f): State<Shared>, Json(body): Json<Value>| async move {
                let mut f = f.lock().unwrap();
                f.posts.push(body);
                Json(json!({ "code": f.feishu_code, "msg": "fake", "data": {} }))
            }),
        )
        .route(
            "/ping/{*rest}",
            get(|State(f): State<Shared>, Path(rest): Path<String>| async move {
                f.lock().unwrap().pings.push(rest);
            }),
        )
        .with_state(fake);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

/// Built without validation, which would ask for https.
fn config(base: &str, webhook: &str, dir: &std::path::Path) -> Config {
    config_at(base, webhook, &dir.join("state.json"))
}

fn config_at(base: &str, webhook: &str, state: &std::path::Path) -> Config {
    toml::from_str(&format!(
        r#"
server = "{base}"
token = "reader"
state = "{}"
[feishu]
webhook = "{webhook}"
secret = "sign-s3cret"
[healthchecks]
ping_url = "{base}/ping/uuid-s3cret"
"#,
        state.display()
    ))
    .unwrap()
}

fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("skym-notify-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn at(minutes: i64) -> Timestamp {
    Timestamp::from_second(1_790_000_000).unwrap() + SignedDuration::from_mins(minutes)
}

fn down() -> IncidentView {
    IncidentView {
        opened_at: Some(at(0)),
        detail: "exited (1)".into(),
        ..incident("workload:x/shop/api", IncidentCode::WorkloadDown, Severity::Critical)
    }
}

fn text(post: &Value) -> &str {
    post["content"]["text"].as_str().unwrap()
}

#[tokio::test]
async fn a_problem_is_told_once_then_resolved_after_the_hold_and_survives_a_restart() {
    let fake = Shared::default();
    fake.lock().unwrap().incidents = vec![down()];
    let base = serve(fake.clone()).await;
    let dir = temp_dir("told");
    let cfg = || config(&base, &format!("{base}/open-apis/bot/v2/hook/hook-s3cret"), &dir);
    let mut n = Notifier::new(cfg()).unwrap();

    n.pass(at(1)).await.unwrap();
    n.pass(at(2)).await.unwrap();
    {
        let f = fake.lock().unwrap();
        assert_eq!(f.posts.len(), 1, "told once");
        assert!(text(&f.posts[0]).contains("🔴 critical  x · shop/api — WORKLOAD_DOWN"));
        assert_eq!(f.posts[0]["timestamp"], at(1).as_second().to_string());
        assert!(f.posts[0]["sign"].is_string());
    }

    let mut restarted = Notifier::new(cfg()).unwrap();
    restarted.pass(at(3)).await.unwrap();
    assert_eq!(fake.lock().unwrap().posts.len(), 1, "remembered across a restart");

    fake.lock().unwrap().incidents.clear();
    restarted.pass(at(4)).await.unwrap();
    restarted.pass(at(13)).await.unwrap();
    assert_eq!(fake.lock().unwrap().posts.len(), 1, "held");
    restarted.pass(at(14)).await.unwrap();
    let f = fake.lock().unwrap();
    assert_eq!(f.posts.len(), 2);
    assert!(text(&f.posts[1]).contains("✅ resolved  x · shop/api"), "{}", text(&f.posts[1]));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn a_refused_message_is_sent_again_and_told_to_healthchecks_after_ten_minutes() {
    let fake = Shared::default();
    fake.lock().unwrap().incidents = vec![down()];
    fake.lock().unwrap().feishu_code = 19021;
    let base = serve(fake.clone()).await;
    let dir = temp_dir("refused");
    let mut n =
        Notifier::new(config(&base, &format!("{base}/open-apis/bot/v2/hook/hook-s3cret"), &dir))
            .unwrap();

    let err = format!("{:#}", n.pass(at(0)).await.unwrap_err());
    assert!(err.contains("19021") && !err.contains("s3cret"), "{err}");
    assert!(!dir.join("state.json").exists(), "nothing remembered");
    assert!(n.pass(at(10)).await.is_err());
    assert_eq!(n.health(at(10)), Some(Ping::Fail));
    n.ping(at(10)).await.unwrap();

    fake.lock().unwrap().feishu_code = 0;
    n.pass(at(11)).await.unwrap();
    assert_eq!(n.health(at(11)), Some(Ping::Ok));
    n.ping(at(11)).await.unwrap();
    let f = fake.lock().unwrap();
    assert_eq!(f.posts.len(), 3, "sent until delivered");
    assert_eq!(f.pings, ["uuid-s3cret/fail", "uuid-s3cret"]);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn an_unreachable_webhook_or_skym_never_shows_a_secret_and_stops_the_pings() {
    let fake = Shared::default();
    fake.lock().unwrap().incidents = vec![down()];
    let base = serve(fake.clone()).await;
    let dir = temp_dir("unreachable");
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
    let webhook = format!("http://{closed}/open-apis/bot/v2/hook/hook-s3cret");
    let mut n = Notifier::new(config(&base, &webhook, &dir)).unwrap();
    let err = format!("{:#}", n.pass(at(0)).await.unwrap_err());
    assert!(!err.contains("s3cret") && !err.contains(&closed.to_string()), "{err}");
    assert_eq!(n.health(at(0)), Some(Ping::Ok), "skym was read");
    assert_eq!(n.health(at(0) + SignedDuration::from_secs(30)), Some(Ping::Ok));

    let mut lost = Notifier::new(config(&format!("http://{closed}"), &webhook, &dir)).unwrap();
    assert!(lost.pass(at(0)).await.is_err());
    assert_eq!(lost.health(at(0)), None, "never read");
    assert_eq!(n.health(at(0) + SignedDuration::from_secs(31)), None, "not read for two passes");
    lost.ping(at(0)).await.unwrap();
    assert!(fake.lock().unwrap().pings.is_empty());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn news_gone_while_feishu_failed_is_no_failure() {
    let fake = Shared::default();
    fake.lock().unwrap().incidents = vec![down()];
    fake.lock().unwrap().feishu_code = 19021;
    let base = serve(fake.clone()).await;
    let dir = temp_dir("gone");
    let mut n =
        Notifier::new(config(&base, &format!("{base}/open-apis/bot/v2/hook/h"), &dir)).unwrap();
    assert!(n.pass(at(0)).await.is_err());
    fake.lock().unwrap().incidents.clear();
    n.pass(at(1)).await.unwrap();
    assert_eq!(n.health(at(11)), None, "not read lately");
    n.pass(at(11)).await.unwrap();
    assert_eq!(n.health(at(11)), Some(Ping::Ok));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn a_failed_save_is_retried_without_telling_twice() {
    let fake = Shared::default();
    fake.lock().unwrap().incidents = vec![down()];
    let base = serve(fake.clone()).await;
    let dir = temp_dir("save");
    let state = dir.join("missing/state.json");
    let mut n = Notifier::new(config_at(&base, &format!("{base}/open-apis/bot/v2/hook/h"), &state))
        .unwrap();
    assert!(n.pass(at(0)).await.is_err(), "the directory is missing");
    std::fs::create_dir(dir.join("missing")).unwrap();
    n.pass(at(1)).await.unwrap();
    assert!(state.exists());
    assert_eq!(fake.lock().unwrap().posts.len(), 1);
    std::fs::remove_dir_all(&dir).unwrap();
}
