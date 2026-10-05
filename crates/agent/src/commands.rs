//! Local commands. None of them use the network beyond the Docker socket and datastore probes.

use crate::collect;
use crate::config::Config;
use crate::exceptions::Detail;
use crate::memory::Memory;
use crate::{report, status};
use jiff::{SignedDuration, Timestamp};
use skym_core::judge::{JudgeInput, Recent, judge};
use skym_core::model::{ExceptionGroup, RunState};
use skym_core::report::{Report, validate};
use skym_core::subject::WorkloadKey;
use skym_core::time::parse_since;
use skym_core::view::{HostView, Status};
use std::collections::{BTreeMap, BTreeSet};

pub async fn status(cfg: &Config, json: bool) -> anyhow::Result<u8> {
    let now = Timestamp::now();
    let (host, c) =
        collect::once(cfg, now, now - SignedDuration::from_mins(15), Detail::Report).await;
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
    let (_, c) = collect::once(cfg, now, parse_since(since, now)?, Detail::Local).await;
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

/// The report a pass would send. Rates are measured over the second after it, so they show
/// the host and its workloads, not this pass's own work.
pub async fn report_dry_run(cfg: &Config) -> anyhow::Result<u8> {
    let now = Timestamp::now();
    let (host, mut c) = collect::once(cfg, now, now - cfg.interval, Detail::Report).await;
    let before = c.counters.clone();
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    (c.counters, _) = collect::host::counters(&c.targets);
    Memory::starting_from(before).rates(&mut c);
    let running: Vec<&WorkloadKey> =
        c.workloads.iter().filter(|w| w.state.run == RunState::Running).map(|w| &w.key).collect();
    let read = running.iter().filter(|k| c.counters.workloads.contains_key(**k)).count();
    eprintln!("cpu: {read}/{} running workloads had their CPU time read", running.len());
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
