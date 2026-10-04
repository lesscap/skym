//! HTTP: `POST /api/report` for hosts, read-only `GET /api/*` for readers.
//! A host token can only report; a reader token can only read.

pub mod apps;
mod handlers;
mod snapshot;
pub mod views;

use crate::config::{Endpoint, ServerConfig, sha256_hex};
use crate::store::Store;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use jiff::Timestamp;
use skym_core::subject::HostId;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use tower_http::decompression::RequestDecompressionLayer;
use tower_http::limit::RequestBodyLimitLayer;

#[derive(Clone)]
pub struct AppState {
    pub store: Store,
    pub cfg: Arc<ServerConfig>,
    /// Token hash → host id, token hash → reader name.
    pub hosts: Arc<HashMap<String, HostId>>,
    pub readers: Arc<HashMap<String, String>>,
    pub started: Timestamp,
    /// Every URL probed (`ServerConfig::probed`), worked out once.
    pub probed: Arc<Vec<Endpoint>>,
}

impl AppState {
    pub fn new(store: Store, cfg: ServerConfig, started: Timestamp) -> Self {
        let hosts = cfg.hosts.iter().map(|h| (h.token_sha256.clone(), h.id.clone())).collect();
        let readers =
            cfg.readers.iter().map(|r| (r.token_sha256.clone(), r.name.clone())).collect();
        AppState {
            store,
            probed: Arc::new(cfg.probed()),
            cfg: Arc::new(cfg),
            hosts: Arc::new(hosts),
            readers: Arc::new(readers),
            started,
        }
    }
}

pub fn router(state: AppState) -> Router {
    let report = Router::new()
        .route("/api/report", post(handlers::report))
        // Authenticate before the body is read: no token, no decompression.
        .route_layer(middleware::from_fn_with_state(state.clone(), require_host))
        .layer(DefaultBodyLimit::max(32 << 20)) // decompressed JSON
        .layer(RequestDecompressionLayer::new())
        .layer(RequestBodyLimitLayer::new(2 << 20)); // compressed body
    let read = Router::new()
        .route("/api", get(handlers::index))
        .route("/api/overview", get(handlers::overview))
        .route("/api/hosts", get(handlers::hosts))
        .route("/api/hosts/{host}", get(handlers::host))
        .route("/api/hosts/{host}/workloads/{project}/{service}", get(handlers::workload))
        .route("/api/timeline", get(handlers::timeline))
        .route("/api/incidents", get(handlers::incidents))
        .route("/api/exceptions", get(handlers::exceptions))
        .route("/api/apps", get(handlers::apps))
        .route("/api/apps/{host}/{project}", get(handlers::app))
        .route("/api/apps/{host}/{project}/{service}", get(handlers::lone_app))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_reader));
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .merge(report)
        .merge(read)
        .with_state(state)
}

fn token_hash(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    Some(sha256_hex(value.strip_prefix("Bearer ")?.trim()))
}

/// The host a report comes from, as its token says.
#[derive(Clone)]
pub struct ReportingHost(pub HostId);

/// The address a report comes from, as far as the server can tell.
#[derive(Clone)]
pub struct ReportingAddr(pub Option<String>);

/// The client's address: the proxy's `X-Real-IP` when the peer is a proxy on this machine or
/// a private network (the reverse proxy in front), else the peer itself, so a client on the
/// internet cannot claim another address.
pub fn client_ip(peer: Option<IpAddr>, forwarded: Option<&str>) -> Option<String> {
    let peer = peer.map(|p| p.to_canonical());
    let trusted = peer.is_some_and(|p| match p {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private(),
        IpAddr::V6(v6) => v6.is_loopback() || v6.is_unique_local(),
    });
    let header = forwarded.and_then(|f| f.trim().parse::<IpAddr>().ok());
    match (trusted, header) {
        (true, Some(ip)) => Some(ip.to_string()),
        _ => peer.map(|p| p.to_string()),
    }
}

async fn require_host(State(state): State<AppState>, mut request: Request, next: Next) -> Response {
    match token_hash(request.headers()).and_then(|h| state.hosts.get(&h).cloned()) {
        Some(host) => {
            let peer = request.extensions().get::<ConnectInfo<SocketAddr>>().map(|c| c.0.ip());
            let forwarded = request.headers().get("x-real-ip").and_then(|v| v.to_str().ok());
            let addr = client_ip(peer, forwarded);
            request.extensions_mut().insert(ReportingHost(host));
            request.extensions_mut().insert(ReportingAddr(addr));
            next.run(request).await
        }
        None => ApiError::unauthorized().into_response(),
    }
}

async fn require_reader(State(state): State<AppState>, request: Request, next: Next) -> Response {
    match token_hash(request.headers()).and_then(|h| state.readers.get(&h).cloned()) {
        Some(reader) => {
            tracing::debug!(reader, path = %request.uri().path(), "read");
            next.run(request).await
        }
        None => ApiError::unauthorized().into_response(),
    }
}

#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    error: &'static str,
    message: String,
}

impl ApiError {
    pub fn bad_request(message: impl Into<String>) -> Self {
        ApiError { status: StatusCode::BAD_REQUEST, error: "bad_request", message: message.into() }
    }

    pub fn unauthorized() -> Self {
        let message = "a valid token is required".into();
        ApiError { status: StatusCode::UNAUTHORIZED, error: "unauthorized", message }
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        ApiError { status: StatusCode::NOT_FOUND, error: "not_found", message: message.into() }
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        tracing::error!("{e:#}");
        let message = "internal error".into();
        ApiError { status: StatusCode::INTERNAL_SERVER_ERROR, error: "internal", message }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = serde_json::json!({ "error": self.error, "message": self.message });
        (self.status, Json(body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::client_ip;

    #[test]
    fn only_a_nearby_proxy_may_name_the_client() {
        let ip = |peer: Option<&str>, header| client_ip(peer.map(|p| p.parse().unwrap()), header);
        assert_eq!(ip(Some("172.18.0.5"), Some("8.149.234.78")).as_deref(), Some("8.149.234.78"));
        assert_eq!(ip(Some("127.0.0.1"), Some(" 2001:db8::1 ")).as_deref(), Some("2001:db8::1"));
        assert_eq!(ip(Some("::1"), Some("8.8.8.8")).as_deref(), Some("8.8.8.8"));
        assert_eq!(ip(Some("fd00::5"), Some("8.8.8.8")).as_deref(), Some("8.8.8.8"));
        let mapped = ip(Some("::ffff:172.18.0.5"), Some("8.8.8.8"));
        assert_eq!(mapped.as_deref(), Some("8.8.8.8"), "a proxy seen through an IPv6 socket");
        let public = ip(Some("::ffff:203.0.113.9"), Some("8.8.8.8"));
        assert_eq!(public.as_deref(), Some("203.0.113.9"), "stored as plain IPv4");
        assert_eq!(
            ip(Some("203.0.113.9"), Some("8.8.8.8")).as_deref(),
            Some("203.0.113.9"),
            "a client on the internet cannot claim another address"
        );
        assert_eq!(ip(Some("2001:db8::9"), Some("8.8.8.8")).as_deref(), Some("2001:db8::9"));
        assert_eq!(ip(Some("10.0.0.2"), Some("not an address")).as_deref(), Some("10.0.0.2"));
        assert_eq!(ip(Some("10.0.0.2"), None).as_deref(), Some("10.0.0.2"));
        assert_eq!(ip(None, Some("8.8.8.8")), None, "no peer, no trust");
    }
}
