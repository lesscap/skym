//! `skym status`: findings of one local pass as a `HostView`, the same shape the server serves.

use skym_core::report::Report;
use skym_core::rules::{Finding, Severity};
use skym_core::subject::Subject;
use skym_core::view::{HostView, IncidentView, Status, WorkloadSummary, rollup};
use std::collections::BTreeMap;

pub fn host_view(report: &Report, mut findings: Vec<Finding>) -> HostView {
    findings.sort_by(|a, b| b.severity.cmp(&a.severity).then_with(|| a.subject.cmp(&b.subject)));
    let incidents: Vec<IncidentView> = findings.into_iter().map(incident).collect();
    let status_of = |subject: &Subject| {
        let own: Vec<IncidentView> =
            incidents.iter().filter(|i| i.subject == *subject).cloned().collect();
        rollup(&own)
    };
    let workloads = report
        .workloads
        .iter()
        .map(|w| WorkloadSummary {
            key: w.key.clone(),
            kind: w.facts.as_ref().map(|f| f.kind),
            status: status_of(&Subject::Workload(w.key.clone())),
            run: w.state.run,
            image: w.facts.as_ref().map(|f| f.image.clone()),
            links: BTreeMap::new(),
        })
        .collect();
    HostView {
        id: report.host.clone(),
        customer: None,
        status: rollup(&incidents),
        last_report_ago: None,
        facts: report.host_facts.clone(),
        state: Some(report.host_state.clone()),
        workloads,
        incidents,
        errors: report.errors.clone(),
    }
}

fn incident(f: Finding) -> IncidentView {
    IncidentView {
        subject: f.subject,
        code: f.code,
        severity: f.severity,
        detail: f.detail,
        opened_at: None,
        open_for: None,
        resolved_at: None,
        muted: false,
        mute_reason: None,
        links: BTreeMap::new(),
    }
}

/// Nagios convention. Anything that could not be observed is at least a warning:
/// a silent gap must not read as healthy.
pub fn exit_code(status: Status, partly_failed: bool, all_failed: bool) -> u8 {
    let code = match status {
        Status::Ok => 0,
        Status::Warn => 1,
        Status::Critical => 2,
        Status::Unknown => 3,
    };
    match (all_failed, partly_failed) {
        (true, _) => 3,
        (false, true) => code.max(1),
        (false, false) => code,
    }
}

pub fn render(view: &HostView) -> String {
    let count = |s| view.incidents.iter().filter(|i| i.severity == s).count();
    let summary = match (view.status, count(Severity::Critical), count(Severity::Warn)) {
        (Status::Unknown, ..) => "unknown: nothing could be collected".to_string(),
        (_, 0, 0) => format!("ok · {} workloads", view.workloads.len()),
        (_, c, w) => format!("{c} critical · {w} warn"),
    };
    let width = view.incidents.iter().map(|i| short(&i.subject).len()).max().unwrap_or(0);
    let rows = view.incidents.iter().map(|i| {
        let level = if i.severity == Severity::Critical { "CRIT" } else { "WARN" };
        let code = i.code.as_str();
        format!("{level:<5} {:<width$}  {code:<22} {}", short(&i.subject), i.detail)
    });
    std::iter::once(format!("host {} · {summary}", view.id))
        .chain(rows)
        .collect::<Vec<_>>()
        .join("\n")
}

/// The subject as a person reads it on this host.
fn short(subject: &Subject) -> String {
    match subject {
        Subject::Workload(k) if k.project == "-" || k.project == "_systemd" => k.service.clone(),
        Subject::Workload(k) => format!("{}/{}", k.project, k.service),
        Subject::Mount { path, .. } => path.clone(),
        Subject::Host(_) => "host".into(),
        Subject::Endpoint(url) => url.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use skym_core::rules::IncidentCode;
    use skym_core::subject::WorkloadKey;

    fn report() -> Report {
        let path =
            format!("{}/../core/tests/fixtures/report-full.json", env!("CARGO_MANIFEST_DIR"));
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    fn finding(subject: Subject, code: IncidentCode, severity: Severity) -> Finding {
        Finding { subject, code, severity, detail: "d".into() }
    }

    #[test]
    fn workload_status_comes_from_its_own_findings() {
        let r = report();
        let app = Subject::Workload(r.workloads[0].key.clone());
        let mount = Subject::Mount { host: "x".into(), path: "/".into() };
        let view = host_view(
            &r,
            vec![
                finding(mount.clone(), IncidentCode::DiskFilling, Severity::Warn),
                finding(app.clone(), IncidentCode::WorkloadDown, Severity::Critical),
            ],
        );
        assert_eq!(view.status, Status::Critical);
        assert_eq!(view.incidents[0].subject, app, "critical first");
        let statuses: Vec<Status> = view.workloads.iter().map(|w| w.status).collect();
        assert_eq!(statuses, [Status::Critical, Status::Ok, Status::Ok]);
        assert!(render(&view).starts_with("host x · 1 critical · 1 warn"));
    }

    #[test]
    fn render_names_subjects_as_a_person_reads_them() {
        let r = report();
        let key = |i: usize| Subject::Workload(r.workloads[i].key.clone());
        let mount = Subject::Mount { host: "x".into(), path: "/data".into() };
        let findings = vec![
            finding(key(0), IncidentCode::WorkloadDown, Severity::Critical),
            finding(key(2), IncidentCode::WorkloadDown, Severity::Warn),
            finding(mount, IncidentCode::DiskFilling, Severity::Warn),
            finding(Subject::Host("x".into()), IncidentCode::OomKilled, Severity::Warn),
        ];
        let text = render(&host_view(&r, findings));
        assert!(text.starts_with("host x · 1 critical · 3 warn"), "{text}");
        let rows: Vec<Vec<&str>> =
            text.lines().skip(1).map(|l| l.split_whitespace().take(3).collect()).collect();
        assert_eq!(
            rows,
            [
                ["CRIT", "captain/api", "WORKLOAD_DOWN"],
                ["WARN", "host", "OOM_KILLED"],
                ["WARN", "xray", "WORKLOAD_DOWN"],
                ["WARN", "/data", "DISK_FILLING"],
            ]
        );
        let plain = WorkloadKey { host: "x".into(), project: "-".into(), service: "legacy".into() };
        assert_eq!(short(&Subject::Workload(plain)), "legacy");
        assert_eq!(render(&host_view(&r, vec![])), "host x · ok · 3 workloads");
    }

    #[test]
    fn exit_codes_never_hide_gaps() {
        assert_eq!(exit_code(Status::Ok, false, false), 0);
        assert_eq!(exit_code(Status::Warn, false, false), 1);
        assert_eq!(exit_code(Status::Critical, false, false), 2);
        assert_eq!(exit_code(Status::Ok, true, false), 1);
        assert_eq!(exit_code(Status::Critical, true, false), 2);
        assert_eq!(exit_code(Status::Ok, true, true), 3);
    }
}
