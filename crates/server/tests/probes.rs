//! Probes against a local HTTP server, and probe passes turning into endpoint incidents.

use axum::Router;
use axum::extract::{Path, Query};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::get;
use jiff::{SignedDuration, Timestamp};
use skym_core::rules::IncidentCode;
use skym_server::config::{Endpoint, Headers};
use skym_server::db;
use skym_server::evaluate::probes_once;
use skym_server::lifecycle::State;
use skym_server::probe::{Probe, Prober};
use skym_server::store::{incidents, probes};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

fn endpoint(url: &str, headers: &[(&str, &str)]) -> Endpoint {
    Endpoint {
        url: url.into(),
        expect: vec![],
        headers: Headers(headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()),
        app: "x/shop".parse().unwrap(),
    }
}

/// A server on 127.0.0.1; `/elsewhere` counts the requests that reach it, `/away?to=…` sends
/// the client there.
async fn serve() -> (String, Arc<AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let elsewhere = Arc::new(AtomicUsize::new(0));
    let hits = elsewhere.clone();
    let app = Router::new()
        .route("/ok", get(|| async { "ok" }))
        .route("/down", get(|| async { StatusCode::SERVICE_UNAVAILABLE }))
        .route(
            "/token",
            get(|h: HeaderMap| async move {
                match h.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()) {
                    Some("Bearer t0ken") => StatusCode::OK,
                    _ => StatusCode::UNAUTHORIZED,
                }
            }),
        )
        .route("/slow", get(|| tokio::time::sleep(Duration::from_secs(5))))
        .route("/here", get(|| async { (StatusCode::FOUND, [(header::LOCATION, "/ok")]) }))
        .route("/loop", get(|| async { (StatusCode::FOUND, [(header::LOCATION, "/loop")]) }))
        .route(
            "/hops/{n}",
            get(|Path(n): Path<u32>| async move {
                match n {
                    0 => "arrived".into_response(),
                    n => (StatusCode::FOUND, [(header::LOCATION, format!("/hops/{}", n - 1))])
                        .into_response(),
                }
            }),
        )
        .route(
            "/away",
            get(|Query(to): Query<BTreeMap<String, String>>| async move {
                (StatusCode::FOUND, [(header::LOCATION, to["to"].clone())]).into_response()
            }),
        )
        .route(
            "/elsewhere",
            get(move || async move {
                hits.fetch_add(1, Ordering::SeqCst);
                "elsewhere"
            }),
        );
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://127.0.0.1:{port}"), elsewhere)
}

#[tokio::test]
async fn probes_see_statuses_headers_timeouts_and_refusals() {
    let (base, _) = serve().await;
    let prober = Prober::new(Duration::from_millis(500)).unwrap();
    let probe = async |path: &str, headers: &[(&str, &str)]| {
        prober.probe(&endpoint(&format!("{base}{path}"), headers)).await.response
    };
    assert_eq!(probe("/ok", &[]).await, Ok(200));
    assert_eq!(probe("/down", &[]).await, Ok(503));
    assert_eq!(probe("/token", &[]).await, Ok(401));
    assert_eq!(probe("/token", &[("Authorization", "Bearer t0ken")]).await, Ok(200));
    assert_eq!(probe("/here", &[]).await, Ok(200), "a redirect on the same host is followed");
    assert_eq!(probe("/hops/5", &[]).await, Ok(200), "five redirects are followed");
    assert_eq!(probe("/hops/6", &[]).await, Err("too many redirects".into()));
    assert_eq!(probe("/slow", &[]).await, Err("timeout after 500ms".into()));

    let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = closed.local_addr().unwrap().port();
    drop(closed);
    let refused = prober.probe(&endpoint(&format!("http://127.0.0.1:{port}/"), &[])).await;
    let why = refused.response.unwrap_err();
    assert!(why.starts_with("connect: ") && why.to_lowercase().contains("refused"), "{why}");
    assert_eq!(refused.cert_not_after, None);
}

#[tokio::test]
async fn a_token_never_follows_a_redirect_to_another_origin() {
    let (base, _) = serve().await;
    let (other, elsewhere) = serve().await;
    let other_host = other.replace("127.0.0.1", "localhost");
    let prober = Prober::new(Duration::from_secs(2)).unwrap();
    for to in [format!("{other}/elsewhere"), format!("{other_host}/elsewhere")] {
        let away = endpoint(&format!("{base}/away?to={to}"), &[("X-Probe-Token", "t0ken")]);
        assert_eq!(prober.probe(&away).await.response, Ok(302), "the 302 is the answer: {to}");
    }
    assert_eq!(elsewhere.load(Ordering::SeqCst), 0, "another port or host is never asked");
}

#[tokio::test]
async fn all_probes_every_endpoint_in_order() {
    let (base, _) = serve().await;
    let prober = Arc::new(Prober::new(Duration::from_secs(2)).unwrap());
    let endpoints: Vec<Endpoint> =
        ["/down", "/ok", "/down"].iter().map(|p| endpoint(&format!("{base}{p}"), &[])).collect();
    let got: Vec<_> =
        prober.all(&endpoints).await.into_iter().map(|(e, p)| (e.url, p.response)).collect();
    let want: Vec<_> =
        endpoints.iter().map(|e| e.url.clone()).zip([Ok(503), Ok(200), Ok(503)]).collect();
    assert_eq!(got, want);
}

fn t0() -> Timestamp {
    "2026-10-01T12:00:00Z".parse().unwrap()
}

fn answered(status: Result<u16, &str>, at: Timestamp, cert_days: Option<i64>) -> Probe {
    Probe {
        at,
        response: status.map_err(str::to_string),
        latency_ms: 30,
        cert_not_after: cert_days.map(|d| at + SignedDuration::from_hours(24 * d)),
    }
}

fn open(c: &rusqlite::Connection) -> BTreeMap<(String, IncidentCode), State> {
    incidents::active_endpoints(c)
        .unwrap()
        .into_iter()
        .map(|i| ((i.subject.to_string(), i.code), i.state))
        .collect()
}

#[test]
fn probe_passes_open_resolve_and_retire_endpoint_incidents() {
    let mut c = db::open_in_memory().unwrap();
    let shop = [endpoint("https://shop.example.com/", &[])];
    let pass = |c: &mut rusqlite::Connection, endpoints: &[Endpoint], p: Probe| {
        let at = p.at;
        let probed: Vec<_> = endpoints.iter().map(|e| (e.clone(), p.clone())).collect();
        probes_once(c, endpoints, &probed, at).unwrap();
    };
    let minute = |m: i64| t0() + SignedDuration::from_mins(m);
    let down = ("endpoint:https://shop.example.com/".to_string(), IncidentCode::EndpointDown);
    let cert = ("endpoint:https://shop.example.com/".to_string(), IncidentCode::CertExpiring);

    pass(&mut c, &shop, answered(Ok(503), minute(0), Some(10)));
    assert_eq!(
        open(&c),
        BTreeMap::from([(down.clone(), State::Pending), (cert.clone(), State::Open)])
    );
    pass(&mut c, &shop, answered(Ok(503), minute(1), Some(10)));
    assert_eq!(open(&c)[&down], State::Open, "two bad probes open it");

    pass(&mut c, &shop, answered(Err("timeout after 10s"), minute(2), None));
    assert_eq!(open(&c)[&cert], State::Open, "no answer, no certificate: still expiring");

    pass(&mut c, &shop, answered(Ok(200), minute(3), Some(90)));
    assert_eq!(open(&c)[&down], State::Open, "one good probe is not enough");
    assert!(!open(&c).contains_key(&cert), "a renewed certificate resolves at once");
    pass(&mut c, &shop, answered(Ok(200), minute(4), Some(90)));
    assert!(open(&c).is_empty(), "two good probes resolve it");

    let row = &probes::all(&c).unwrap()[0];
    assert_eq!(
        (row.first_seen, row.probe.at, row.probe.response.clone()),
        (minute(0), minute(4), Ok(200))
    );

    pass(&mut c, &shop, answered(Ok(500), minute(5), None));
    pass(&mut c, &shop, answered(Ok(500), minute(6), None));
    assert_eq!(open(&c)[&down], State::Open);
    probes_once(&mut c, &shop, &[], minute(7)).unwrap();
    probes_once(&mut c, &shop, &[], minute(8)).unwrap();
    assert_eq!(open(&c)[&down], State::Open, "not probed is not recovered");
    assert_eq!(probes::all(&c).unwrap().len(), 1, "nor forgotten");
    pass(&mut c, &[], answered(Ok(200), minute(9), None));
    assert!(open(&c).is_empty(), "an endpoint removed from the configuration is retired");
    assert!(probes::all(&c).unwrap().is_empty(), "and forgotten");
}
