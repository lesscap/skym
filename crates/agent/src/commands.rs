//! Local commands. None of them use the network beyond the Docker socket and datastore probes.

use crate::collect::{Collected, Window, collect, host};
use crate::config::Config;
use crate::exceptions::Detail;
use crate::{report, status};
use jiff::{SignedDuration, Timestamp};
use skym_core::judge::{JudgeInput, Recent, judge};
use skym_core::model::ExceptionGroup;
use skym_core::report::{Report, validate};
use skym_core::time::parse_since;
use skym_core::view::{HostView, Status};
use std::collections::{BTreeMap, BTreeSet};

/// One collection pass: events from the last hour, logs after `logs_after`.
/// Collection errors go to stderr.
pub async fn pass(
    cfg: &Config,
    now: Timestamp,
    logs_after: Timestamp,
    detail: Detail,
) -> (String, Collected) {
    let host = host::hostname();
    let events_since = now - SignedDuration::from_hours(1);
    let cursors = BTreeMap::new();
    let window = Window { now, events_since, logs_after, cursors: &cursors, detail };
    let collected = collect(cfg, &host, &window).await;
    for e in &collected.errors {
        eprintln!("error: {e}");
    }
    (host, collected)
}

pub async fn status(cfg: &Config, json: bool) -> anyhow::Result<u8> {
    let now = Timestamp::now();
    let (host, c) = pass(cfg, now, now - SignedDuration::from_mins(15), Detail::Report).await;
    let report = report::build(&c, &host, now);
    if let Err(e) = validate(&report) {
        eprintln!("error: invalid report: {e}");
        return Ok(3);
    }
    let input = JudgeInput {
        report: &report,
        facts: &BTreeMap::new(),
        recent: Recent { oom_events: &report.local_events, exceptions: &report.exceptions },
        open: &BTreeSet::new(),
    };
    let mut view = status::host_view(&report, judge(&input));
    if c.all_failed {
        view.status = Status::Unknown;
    }
    match json {
        true => println!("{}", serde_json::to_string_pretty(&view)?),
        false => println!("{}", status::render(&view)),
    }
    Ok(status::exit_code(view.status, !c.errors.is_empty(), c.all_failed))
}

pub async fn exceptions(
    cfg: &Config,
    workload: Option<&str>,
    since: &str,
    json: bool,
) -> anyhow::Result<u8> {
    let now = Timestamp::now();
    let (_, c) = pass(cfg, now, parse_since(since, now)?, Detail::Local).await;
    let wanted = |g: &&ExceptionGroup| {
        workload.is_none_or(|w| format!("{}/{}", g.workload.project, g.workload.service) == w)
    };
    let groups: Vec<&ExceptionGroup> = c.exceptions.iter().filter(wanted).collect();
    match json {
        true => println!("{}", serde_json::to_string_pretty(&groups)?),
        false => groups.iter().for_each(|g| println!("{}", render_group(g))),
    }
    Ok(status::exit_code(Status::Ok, !c.errors.is_empty(), c.all_failed))
}

fn render_group(g: &ExceptionGroup) -> String {
    let sample = g.sample.as_ref();
    let message = sample.map(|s| s.message.as_str()).unwrap_or_default();
    let stack = sample.and_then(|s| s.stacktrace.as_deref()).unwrap_or_default();
    let head = format!(
        "{}/{} {} {} ×{} (gave up {}) last {}",
        g.workload.project,
        g.workload.service,
        g.component,
        g.code,
        g.count,
        g.final_count,
        g.last_seen
    );
    [head, indent(message), indent(stack)]
        .into_iter()
        .filter(|s| !s.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn indent(s: &str) -> String {
    s.lines().map(|l| format!("    {l}")).collect::<Vec<_>>().join("\n")
}

pub async fn report_dry_run(cfg: &Config) -> anyhow::Result<u8> {
    let now = Timestamp::now();
    let (host, c) = pass(cfg, now, now - cfg.interval, Detail::Report).await;
    println!("{}", serde_json::to_string_pretty(&report::build(&c, &host, now))?);
    Ok(status::exit_code(Status::Ok, !c.errors.is_empty(), c.all_failed))
}

#[derive(clap::ValueEnum, Clone, Copy, Debug)]
pub enum SchemaKind {
    Status,
    Exceptions,
    Report,
}

pub fn schema(which: SchemaKind) -> anyhow::Result<u8> {
    let schema = match which {
        SchemaKind::Status => schemars::schema_for!(HostView),
        SchemaKind::Exceptions => schemars::schema_for!(Vec<ExceptionGroup>),
        SchemaKind::Report => schemars::schema_for!(Report),
    };
    println!("{}", serde_json::to_string_pretty(&schema)?);
    Ok(0)
}
