//! The agent against a real `skym-server` on a local port. Docker collection is off; on a
//! host without `/proc` (macOS) the host source fails too, which reports must survive.

use super::*;
use crate::config::DockerConfig;
use jiff::SignedDuration;
use skym_server::api::{AppState, router};
use skym_server::config::{HostEntry, ServerConfig, sha256_hex};
use skym_server::db;
use skym_server::store::{Store, hosts};
use std::net::SocketAddr;
use std::path::Path;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

const TOKEN: &str = "host-x-token";

fn state() -> AppState {
    let cfg = ServerConfig {
        hosts: vec![HostEntry { id: "x".into(), token_sha256: sha256_hex(TOKEN), tags: vec![] }],
        ..ServerConfig::default()
    };
    AppState::new(Store::new(db::open_in_memory().unwrap()), cfg, Timestamp::now())
}

/// Serves until the sender is used, then closes every connection.
async fn serve(
    state: &AppState,
    addr: SocketAddr,
) -> (SocketAddr, oneshot::Sender<()>, JoinHandle<()>) {
    let listener = TcpListener::bind(addr).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (stop, stopped) = oneshot::channel::<()>();
    let app = router(state.clone());
    let task = tokio::spawn(async move {
        axum::serve(listener, app)
            // A sent or a dropped sender: either way the test is done with this server.
            .with_graceful_shutdown(async { stopped.await.unwrap_or_default() })
            .await
            .unwrap();
    });
    (addr, stop, task)
}

fn config(dir: &Path, addr: SocketAddr, token: &str) -> Config {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("token"), token).unwrap();
    Config {
        server: Some(format!("http://{addr}")),
        token_file: dir.join("token"),
        state_dir: dir.join("state"),
        docker: DockerConfig { enabled: false, ..DockerConfig::default() },
        ..Config::default()
    }
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("skym-agent-{name}-{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).unwrap(); // left over from an interrupted run
    }
    dir
}

async fn last_report(state: &AppState) -> Option<Timestamp> {
    let row = state.store.call(|c| Ok(hosts::get(c, "x")?)).await.unwrap();
    row.map(|r| r.last_report_ts)
}

#[tokio::test]
async fn a_pass_delivers_its_report() {
    let server = state();
    let (addr, _stop, _task) = serve(&server, "127.0.0.1:0".parse().unwrap()).await;
    let dir = temp("deliver");
    let cfg = config(&dir, addr, TOKEN);
    let mut agent = Agent::new(&cfg).unwrap();
    agent.pass().await;
    assert!(last_report(&server).await.is_some());
    assert!(agent.outbox.pending().unwrap().is_empty());
    assert!(dir.join("state/cursors.json").exists(), "log cursors saved");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn a_report_that_cannot_be_stored_is_sent_at_once() {
    use std::os::unix::fs::PermissionsExt;
    let server = state();
    let (addr, _stop, _task) = serve(&server, "127.0.0.1:0".parse().unwrap()).await;
    let dir = temp("unstored");
    let cfg = config(&dir, addr, TOKEN);
    let mut agent = Agent::new(&cfg).unwrap();
    let outbox = dir.join("state/outbox");
    std::fs::set_permissions(&outbox, std::fs::Permissions::from_mode(0o500)).unwrap();
    assert!(
        std::fs::write(outbox.join("probe"), "").is_err(),
        "the outbox must be unwritable for this test: run the tests as a user other than root"
    );
    agent.pass().await;
    assert!(last_report(&server).await.is_some(), "a full disk does not silence the host");
    std::fs::set_permissions(&outbox, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn reports_wait_while_the_server_is_away_and_are_replayed() {
    let server = state();
    let (addr, stop, task) = serve(&server, "127.0.0.1:0".parse().unwrap()).await;
    let dir = temp("replay");
    let cfg = config(&dir, addr, TOKEN);
    let mut agent = Agent::new(&cfg).unwrap();
    agent.pass().await;
    stop.send(()).unwrap();
    task.await.unwrap();
    for _ in 0..2 {
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await; // distinct seconds
        agent.pass().await;
    }
    assert_eq!(agent.outbox.pending().unwrap().len(), 2, "kept while the server is away");
    let (_, _stop, _task) = serve(&server, addr).await;
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    agent.pass().await;
    assert!(agent.outbox.pending().unwrap().is_empty(), "none left waiting");
    let newest = last_report(&server).await.unwrap();
    assert!(Timestamp::now().duration_since(newest) < SignedDuration::from_secs(2));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn a_rejected_token_keeps_the_report() {
    let server = state();
    let (addr, _stop, _task) = serve(&server, "127.0.0.1:0".parse().unwrap()).await;
    let dir = temp("token");
    let cfg = config(&dir, addr, "not-the-token");
    let mut agent = Agent::new(&cfg).unwrap();
    agent.pass().await;
    assert_eq!(agent.outbox.pending().unwrap().len(), 1);
    assert_eq!(last_report(&server).await, None);
    std::fs::remove_dir_all(&dir).unwrap();
}
