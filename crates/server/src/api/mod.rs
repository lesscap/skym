//! HTTP: `POST /api/report` for hosts, read-only `GET /api/*` for readers.
//! A host token can only report; a reader token can only read.

mod handlers;
pub mod views;

use crate::config::{ServerConfig, sha256_hex};
use crate::store::Store;
use axum::extract::{DefaultBodyLimit, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use jiff::Timestamp;
use skym_core::subject::HostId;
use std::collections::HashMap;
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
}

impl AppState {
    pub fn new(store: Store, cfg: ServerConfig, started: Timestamp) -> Self {
        let hosts = cfg.hosts.iter().map(|h| (h.token_sha256.clone(), h.id.clone())).collect();
        let readers =
            cfg.readers.iter().map(|r| (r.token_sha256.clone(), r.name.clone())).collect();
        AppState {
            store,
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
        .route("/api/hosts/{host}", get(handlers::host))
        .route("/api/hosts/{host}/workloads/{project}/{service}", get(handlers::workload))
        .route("/api/timeline", get(handlers::timeline))
        .route("/api/incidents", get(handlers::incidents))
        .route("/api/exceptions", get(handlers::exceptions))
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

async fn require_host(State(state): State<AppState>, mut request: Request, next: Next) -> Response {
    match token_hash(request.headers()).and_then(|h| state.hosts.get(&h).cloned()) {
        Some(host) => {
            request.extensions_mut().insert(ReportingHost(host));
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
