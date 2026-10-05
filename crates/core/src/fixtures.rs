//! Sample data for other crates' tests: the full sample report,
//! `tests/fixtures/report-full.json`, and views with only what identifies them. A test sets
//! what it asserts on (`AppSummary { status: Warn, ..app("x/shop") }`); a field added later
//! takes its default here, so no test has to change for it.

use crate::report::Report;
use crate::rules::{IncidentCode, Severity};
use crate::view::{AppSummary, HostOverview, IncidentView, WorkloadSummary};
use serde_json::json;

pub fn full_report() -> Report {
    serde_json::from_str(include_str!("../tests/fixtures/report-full.json")).expect("valid fixture")
}

fn from<T: serde::de::DeserializeOwned>(value: serde_json::Value) -> T {
    serde_json::from_value(value).expect("valid fixture")
}

/// An ok application named after its key's label (`shop` for `x/shop`).
pub fn app(key: &str) -> AppSummary {
    let name = key.rsplit('/').next().unwrap_or(key);
    from(json!({ "key": key, "name": name, "status": "ok" }))
}

/// An ok, running workload: `host/project/service`.
pub fn workload(key: &str) -> WorkloadSummary {
    let mut parts = key.splitn(3, '/');
    let (host, project, service) = (parts.next(), parts.next(), parts.next());
    from(json!({
        "key": { "host": host, "project": project, "service": service },
        "status": "ok", "run": "running"
    }))
}

/// An ok host that reported a moment ago.
pub fn host_overview(id: &str) -> HostOverview {
    from(json!({ "id": id, "status": "ok", "last_report_ago": "5s" }))
}

/// An open, unmuted incident with no detail and no application.
pub fn incident(subject: &str, code: IncidentCode, severity: Severity) -> IncidentView {
    from(json!({
        "subject": subject, "code": code, "severity": severity, "detail": "",
        "muted": false
    }))
}
