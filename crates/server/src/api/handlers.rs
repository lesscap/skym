use super::apps;
use super::views::{self, incident_view};
use super::{ApiError, AppState, ReportingHost};
use crate::ingest::{IngestError, MAX_AHEAD, ingest};
use crate::store::hosts::WorkloadRow;
use crate::store::probes::ProbeRow;
use crate::store::{history, hosts, incidents, probes};
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
    AppList, AppSummary, AppView, EndpointOverview, ExceptionList, HostView, IncidentList,
    IncidentView, Overview, Timeline, WorkloadView, workload_summary,
};
use std::collections::BTreeMap;

type ApiResult<T> = Result<Json<T>, ApiError>;

/// Generous bound on rows read for one response; lists are cut to `limit` afterwards.
/// Mutes live in the configuration, so muted incidents are filtered after this bound.
const MAX_ROWS: usize = 10_000;

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
            { "path": "/api/overview", "answers": "where is something wrong right now" },
            { "path": "/api/apps", "answers": "which applications exist, where, and are they up" },
            { "path": "/api/apps/{host}/{project}", "answers": "this application: its note, URLs and services (lone containers and systemd units: /api/apps/{host}/{project}/{service})" },
            { "path": "/api/hosts/{host}", "answers": "what is going on with this host" },
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
    let now = Timestamp::now();
    let (all, probed, open, stopped) = s
        .store
        .call(|c| {
            let open = incidents::listed(c, true, None, None, Timestamp::UNIX_EPOCH, MAX_ROWS)?;
            let stopped = hosts::stopped_since(c, &open)?;
            Ok((hosts::all(c)?, probes::all(c)?, open, stopped))
        })
        .await?;
    let probed = probed.into_iter().map(|p| (p.url.clone(), p)).collect();
    let seen = all
        .into_iter()
        .map(|h| (h.id, views::Seen { first: h.first_seen, last: h.last_seen }))
        .collect();
    let views: Vec<IncidentView> =
        open.iter().map(|i| incident_view(i, &s.cfg.mute, &stopped, now)).collect();
    Ok(Json(views::overview(&s.cfg, &s.probed, &seen, &probed, &views, now)))
}

pub async fn host(State(s): State<AppState>, Path(host): Path<String>) -> ApiResult<HostView> {
    configured(&s, &host)?;
    let now = Timestamp::now();
    let id = host.clone();
    let (row, workloads, open) = s
        .store
        .call(move |c| {
            Ok((
                hosts::get(c, &id)?,
                hosts::workloads(c, &id)?,
                incidents::listed(c, true, Some(&id), None, Timestamp::UNIX_EPOCH, MAX_ROWS)?,
            ))
        })
        .await?;
    let stopped =
        workloads.iter().filter_map(|w| Some((w.key.clone(), w.state.stopped_since()?))).collect();
    let incidents = open.iter().map(|i| incident_view(i, &s.cfg.mute, &stopped, now)).collect();
    let customer = s.cfg.hosts.iter().find(|h| h.id == host).map(|h| h.customer.clone());
    Ok(Json(views::host(host, customer, row, workloads, incidents, now)))
}

pub async fn workload(
    State(s): State<AppState>,
    Path((host, project, service)): Path<(String, String, String)>,
) -> ApiResult<WorkloadView> {
    configured(&s, &host)?;
    let now = Timestamp::now();
    let key = WorkloadKey { host: host.clone(), project, service };
    let subject = Subject::Workload(key.clone());
    let (k, sub) = (key.clone(), subject.clone());
    let (row, open, exceptions, events) = s
        .store
        .call(move |c| {
            let row = hosts::workloads(c, &k.host)?.into_iter().find(|w| w.key == k);
            let open =
                incidents::listed(c, true, Some(&k.host), None, Timestamp::UNIX_EPOCH, MAX_ROWS)?;
            let hour = now - jiff::SignedDuration::from_hours(1);
            let exceptions =
                history::exceptions(c, &k.host, (hour, now + MAX_AHEAD), Some(&k), None)?;
            let day = now - jiff::SignedDuration::from_hours(24);
            Ok((row, open, exceptions, history::events(c, &k.host, Some(&sub), day, 50)?))
        })
        .await?;
    let row = row.ok_or_else(|| ApiError::not_found(format!("workload {subject} is unknown")))?;
    let stopped = row.state.stopped_since().map(|t| (row.key.clone(), t)).into_iter().collect();
    let incidents = open
        .iter()
        .filter(|i| i.subject == subject)
        .map(|i| incident_view(i, &s.cfg.mute, &stopped, now))
        .collect();
    Ok(Json(views::workload(row, incidents, exceptions, events)))
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
    let (rows, stopped) = s
        .store
        .call(move |c| {
            let rows = incidents::listed(c, open, host.as_deref(), code, from, MAX_ROWS)?;
            let stopped = hosts::stopped_since(c, &rows)?;
            Ok((rows, stopped))
        })
        .await?;
    let views: Vec<IncidentView> = rows
        .iter()
        .map(|i| incident_view(i, &s.cfg.mute, &stopped, now))
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

/// Everything the applications are made of, read in one go.
async fn app_data(
    s: &AppState,
    now: Timestamp,
) -> Result<(Vec<WorkloadRow>, Vec<AppSummary>), ApiError> {
    let (workloads, probed, open, deployed) = s
        .store
        .call(|c| {
            let open = incidents::listed(c, true, None, None, Timestamp::UNIX_EPOCH, MAX_ROWS)?;
            Ok((hosts::all_workloads(c)?, probes::all(c)?, open, history::last_deployed(c)?))
        })
        .await?;
    let stopped =
        workloads.iter().filter_map(|w| Some((w.key.clone(), w.state.stopped_since()?))).collect();
    let views: Vec<IncidentView> =
        open.iter().map(|i| incident_view(i, &s.cfg.mute, &stopped, now)).collect();
    let probed: BTreeMap<String, ProbeRow> =
        probed.into_iter().map(|p| (p.url.clone(), p)).collect();
    let endpoints: Vec<EndpointOverview> = s
        .probed
        .iter()
        .filter(|e| e.app.is_some())
        .map(|e| views::endpoint(e, probed.get(&e.url), &views, s.cfg.report_interval, now))
        .collect();
    let apps = apps::summaries(&workloads, &s.cfg.apps, &endpoints, &views, &deployed);
    Ok((workloads, apps))
}

pub async fn apps(State(s): State<AppState>) -> ApiResult<AppList> {
    let (_, apps) = app_data(&s, Timestamp::now()).await?;
    Ok(Json(AppList { apps }))
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
    let (workloads, apps) = app_data(&s, Timestamp::now()).await?;
    let app = apps
        .into_iter()
        .find(|a| a.key == key)
        .ok_or_else(|| ApiError::not_found(format!("app {key} is unknown")))?;
    let workloads = workloads
        .into_iter()
        .filter(|w| key.contains(&w.key))
        .map(|w| {
            let links = views::links(&Subject::Workload(w.key.clone()));
            workload_summary(w.key, w.facts.as_ref(), &w.state, &app.incidents, links)
        })
        .collect();
    Ok(Json(AppView { app, workloads }))
}
