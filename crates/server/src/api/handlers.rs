use super::snapshot::{MAX_ROWS, Snapshot};
use super::views;
use super::{ApiError, AppState, ReportingHost};
use crate::ingest::{IngestError, MAX_AHEAD, ingest};
use crate::lifecycle::Incident;
use crate::store::{history, incidents};
use axum::Json;
use axum::body::Bytes;
use axum::extract::{Extension, Path, Query, State};
use jiff::Timestamp;
use serde::Deserialize;
use serde_json::{Value, json};
use skym_core::model::ExceptionClass;
use skym_core::subject::{AppKey, Subject, WorkloadKey};
use skym_core::time::parse_since;
use skym_core::view::{
    AppList, AppView, ExceptionList, HostList, HostView, IncidentList, IncidentView, Overview,
    Timeline, WorkloadView, workload_summary,
};

type ApiResult<T> = Result<Json<T>, ApiError>;

pub async fn report(
    State(s): State<AppState>,
    Extension(ReportingHost(host)): Extension<ReportingHost>,
    body: Bytes,
) -> ApiResult<Value> {
    let now = Timestamp::now();
    let apps: Vec<_> = s.cfg.apps.iter().filter(|a| a.id.host == host).cloned().collect();
    match s.store.call(move |c| Ok(ingest(c, &host, &apps, &body, now))).await? {
        Ok(_) => Ok(Json(json!({ "ok": true }))),
        Err(IngestError::Invalid(message)) => Err(ApiError::bad_request(message)),
        Err(IngestError::Internal(e)) => Err(e.into()),
    }
}

pub async fn index() -> Json<Value> {
    Json(json!({
        "endpoints": [
            { "path": "/api/overview", "answers": "where is something wrong right now: problems (each with its app, else its host) and hosts" },
            { "path": "/api/apps", "answers": "which applications exist, where, and are they up" },
            { "path": "/api/apps/{host}/{project}", "answers": "this application: its note, URLs and services (lone containers and systemd units: /api/apps/{host}/{project}/{service})" },
            { "path": "/api/hosts", "answers": "every host: status, resources, applications" },
            { "path": "/api/hosts/{host}", "answers": "what is going on with this host, and its applications" },
            { "path": "/api/hosts/{host}/workloads/{project}/{service}", "answers": "what is going on with this application" },
            { "path": "/api/timeline", "params": "host (required), workload (project/service), since (default 6h), limit", "answers": "when did it start, what else happened" },
            { "path": "/api/incidents", "params": "status (open|resolved, default open), host, code, since (resolved only, default 24h), include_muted (default false), limit (default 100, at most 1000)", "answers": "incident history" },
            { "path": "/api/exceptions", "params": "host (required), workload, class (application|business), since (default 1h), limit", "answers": "exception groups" }
        ],
        "notes": "Follow `links` in responses instead of building URLs. `open_for` tells new problems from chronic ones."
    }))
}

fn limit(requested: Option<usize>) -> usize {
    requested.unwrap_or(100).clamp(1, 1000)
}

fn since(requested: Option<&str>, default: &str, now: Timestamp) -> Result<Timestamp, ApiError> {
    parse_since(requested.unwrap_or(default), now).map_err(|e| ApiError::bad_request(e.to_string()))
}

fn configured(s: &AppState, host: &str) -> Result<(), ApiError> {
    match s.cfg.hosts.iter().any(|h| h.id == host) {
        true => Ok(()),
        false => Err(ApiError::not_found(format!("host {host:?} is unknown"))),
    }
}

fn workload_key(host: &str, workload: &str) -> Result<WorkloadKey, ApiError> {
    let (project, service) = workload
        .split_once('/')
        .ok_or_else(|| ApiError::bad_request("workload is project/service"))?;
    Ok(WorkloadKey { host: host.into(), project: project.into(), service: service.into() })
}

pub async fn overview(State(s): State<AppState>) -> ApiResult<Overview> {
    let snap = Snapshot::read(&s).await?;
    let open = snap.views(&s, &snap.open);
    let apps = snap.apps(&s, &open);
    Ok(Json(views::overview(&s.cfg, &snap.hosts, &apps, &open, snap.now)))
}

pub async fn hosts(State(s): State<AppState>) -> ApiResult<HostList> {
    let snap = Snapshot::read(&s).await?;
    let open = snap.views(&s, &snap.open);
    let apps = snap.apps(&s, &open);
    Ok(Json(HostList { hosts: views::hosts(&s.cfg, &snap.hosts, &apps, &open, snap.now) }))
}

pub async fn host(State(s): State<AppState>, Path(host): Path<String>) -> ApiResult<HostView> {
    configured(&s, &host)?;
    let snap = Snapshot::read(&s).await?;
    let open = snap.views(&s, &snap.open);
    let apps = snap.apps(&s, &open);
    let mine: Vec<Incident> =
        snap.open.iter().filter(|i| i.host.as_ref() == Some(&host)).cloned().collect();
    let row = snap.hosts.iter().find(|h| h.id == host).cloned();
    let workloads = snap.workloads.iter().filter(|w| w.key.host == host).cloned().collect();
    let incidents = snap.views(&s, &mine);
    let apps = apps.into_iter().filter(|a| a.key.host == host).collect();
    Ok(Json(views::host(host, row, workloads, incidents, apps, snap.now)))
}

pub async fn workload(
    State(s): State<AppState>,
    Path((host, project, service)): Path<(String, String, String)>,
) -> ApiResult<WorkloadView> {
    configured(&s, &host)?;
    let key = WorkloadKey { host: host.clone(), project, service };
    let subject = Subject::Workload(key.clone());
    let snap = Snapshot::read(&s).await?;
    let row = snap.workloads.iter().find(|w| w.key == key).cloned();
    let row = row.ok_or_else(|| ApiError::not_found(format!("workload {subject} is unknown")))?;
    let now = snap.now;
    let (k, sub) = (key.clone(), subject.clone());
    let (exceptions, events) = s
        .store
        .call(move |c| {
            let hour = now - jiff::SignedDuration::from_hours(1);
            let exceptions =
                history::exceptions(c, &k.host, (hour, now + MAX_AHEAD), Some(&k), None)?;
            let day = now - jiff::SignedDuration::from_hours(24);
            Ok((exceptions, history::events(c, &k.host, Some(&sub), day, 50)?))
        })
        .await?;
    let mine: Vec<Incident> = snap.open.iter().filter(|i| i.subject == subject).cloned().collect();
    Ok(Json(views::workload(row, snap.views(&s, &mine), exceptions, events)))
}

#[derive(Deserialize)]
pub struct TimelineQuery {
    host: Option<String>,
    workload: Option<String>,
    since: Option<String>,
    limit: Option<usize>,
}

pub async fn timeline(
    State(s): State<AppState>,
    Query(q): Query<TimelineQuery>,
) -> ApiResult<Timeline> {
    let host = q.host.ok_or_else(|| ApiError::bad_request("host is required"))?;
    configured(&s, &host)?;
    let now = Timestamp::now();
    let from = since(q.since.as_deref(), "6h", now)?;
    let subject =
        q.workload.as_deref().map(|w| workload_key(&host, w).map(Subject::Workload)).transpose()?;
    let n = limit(q.limit);
    let (changes, events) = s
        .store
        .call(move |c| {
            Ok((
                incidents::changes(c, &host, subject.as_ref(), from, n + 1)?,
                history::events(c, &host, subject.as_ref(), from, n + 1)?,
            ))
        })
        .await?;
    Ok(Json(views::timeline(changes, events, n)))
}

#[derive(Deserialize)]
pub struct IncidentQuery {
    status: Option<String>,
    host: Option<String>,
    code: Option<String>,
    since: Option<String>,
    include_muted: Option<bool>,
    limit: Option<usize>,
}

pub async fn incidents(
    State(s): State<AppState>,
    Query(q): Query<IncidentQuery>,
) -> ApiResult<IncidentList> {
    let now = Timestamp::now();
    let open = match q.status.as_deref().unwrap_or("open") {
        "open" => true,
        "resolved" => false,
        other => return Err(ApiError::bad_request(format!("unknown status {other:?}"))),
    };
    let code = q.code.as_deref().map(str::parse).transpose().map_err(ApiError::bad_request)?;
    let from = since(q.since.as_deref(), "24h", now)?;
    let n = limit(q.limit);
    let host = q.host.clone();
    let rows = s
        .store
        .call(move |c| Ok(incidents::listed(c, open, host.as_deref(), code, from, MAX_ROWS)?))
        .await?;
    let snap = Snapshot::read(&s).await?;
    let views: Vec<IncidentView> = snap
        .views(&s, &rows)
        .into_iter()
        .filter(|i| q.include_muted.unwrap_or(false) || !i.muted)
        .collect();
    let (incidents, truncated) = views::cap(views, n);
    Ok(Json(IncidentList { incidents, truncated }))
}

#[derive(Deserialize)]
pub struct ExceptionQuery {
    host: Option<String>,
    workload: Option<String>,
    class: Option<String>,
    since: Option<String>,
    limit: Option<usize>,
}

pub async fn exceptions(
    State(s): State<AppState>,
    Query(q): Query<ExceptionQuery>,
) -> ApiResult<ExceptionList> {
    let host = q.host.ok_or_else(|| ApiError::bad_request("host is required"))?;
    configured(&s, &host)?;
    let now = Timestamp::now();
    let from = since(q.since.as_deref(), "1h", now)?;
    let key = q.workload.as_deref().map(|w| workload_key(&host, w)).transpose()?;
    let class = match q.class.as_deref().map(str::parse::<ExceptionClass>) {
        None => None,
        Some(Ok(ExceptionClass::Unknown) | Err(_)) => {
            return Err(ApiError::bad_request("class is application or business"));
        }
        Some(Ok(class)) => Some(class),
    };
    let n = limit(q.limit);
    let groups = s
        .store
        .call(move |c| {
            Ok(history::exceptions(c, &host, (from, now + MAX_AHEAD), key.as_ref(), class)?)
        })
        .await?;
    let (exceptions, truncated) = views::cap(groups, n);
    Ok(Json(ExceptionList { exceptions, truncated }))
}

pub async fn apps(State(s): State<AppState>) -> ApiResult<AppList> {
    let snap = Snapshot::read(&s).await?;
    let open = snap.views(&s, &snap.open);
    Ok(Json(AppList { apps: snap.apps(&s, &open) }))
}

pub async fn app(
    State(s): State<AppState>,
    Path((host, project)): Path<(String, String)>,
) -> ApiResult<AppView> {
    one_app(s, AppKey { host, project, service: None }).await
}

pub async fn lone_app(
    State(s): State<AppState>,
    Path((host, project, service)): Path<(String, String, String)>,
) -> ApiResult<AppView> {
    one_app(s, AppKey { host, project, service: Some(service) }).await
}

/// One application with its workloads, each with its own status.
async fn one_app(s: AppState, key: AppKey) -> ApiResult<AppView> {
    let snap = Snapshot::read(&s).await?;
    let open = snap.views(&s, &snap.open);
    let app = snap
        .apps(&s, &open)
        .into_iter()
        .find(|a| a.key == key)
        .ok_or_else(|| ApiError::not_found(format!("app {key} is unknown")))?;
    let workloads = snap
        .workloads
        .into_iter()
        .filter(|w| key.contains(&w.key))
        .map(|w| {
            let links = views::links(&Subject::Workload(w.key.clone()));
            workload_summary(w.key, w.facts.as_ref(), &w.state, &app.incidents, links)
        })
        .collect();
    Ok(Json(AppView { app, workloads }))
}
