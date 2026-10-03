//! `skym doctor`: is this installation ready to report? Ends by sending one real report.

use crate::commands::pass;
use crate::config::Config;
use crate::deliver::{self, Client};
use crate::exceptions::Detail;
use crate::outbox::{self, Outbox};
use crate::report;
use anyhow::Context;
use jiff::Timestamp;
use skym_core::report::Report;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

enum Mark {
    Ok,
    Warn,
    Fail,
}

fn show(mark: Mark, what: &str) -> bool {
    let sign = match mark {
        Mark::Ok => "✓",
        Mark::Warn => "⚠",
        Mark::Fail => "✗",
    };
    println!("{sign} {what}");
    matches!(mark, Mark::Fail)
}

/// Exit code 1 when any check fails.
pub async fn run(cfg: &Config) -> anyhow::Result<u8> {
    let Some(server) = cfg.server.as_deref() else {
        show(Mark::Fail, "server: not set in the configuration");
        return Ok(1);
    };
    show(Mark::Ok, &format!("server: {server}"));
    let token = match deliver::read_token(&cfg.token_file) {
        Ok(token) => token,
        Err(e) => {
            show(Mark::Fail, &format!("token: {e:#}"));
            return Ok(1);
        }
    };
    let mode = std::fs::metadata(&cfg.token_file)?.permissions().mode();
    match mode & 0o077 {
        0 => show(Mark::Ok, &format!("token: {}", cfg.token_file.display())),
        _ => show(
            Mark::Warn,
            &format!("token: {} is readable by others; chmod 600 it", cfg.token_file.display()),
        ),
    };
    let state = &cfg.state_dir;
    let failed_state = match writable(state) {
        Ok(()) => show(Mark::Ok, &format!("state: {} is writable", state.display())),
        Err(e) => show(Mark::Fail, &format!("state: {e:#}")),
    };
    let now = Timestamp::now();
    let (host, c) = pass(cfg, now, now - cfg.interval, Detail::Report).await;
    let collected = format!("collection: {} workloads", c.workloads.len());
    let mut failed = failed_state
        | match (c.all_failed, c.errors.len()) {
            (true, _) => show(Mark::Fail, "collection: every source failed (errors above)"),
            (false, 0) => show(Mark::Ok, &collected),
            (false, n) => {
                show(Mark::Warn, &format!("{collected}; {n} sources failed (errors above)"))
            }
        };
    let client = Client::new(server, token)?;
    match client.healthz().await {
        Ok((200, _)) => show(Mark::Ok, "server reachable"),
        Ok((status, _)) => {
            return Ok(u8::from(show(
                Mark::Fail,
                &format!("server answered /healthz with {status}"),
            )));
        }
        Err(e) => return Ok(u8::from(show(Mark::Fail, &format!("server unreachable: {e}")))),
    };
    // Without exception groups: a running `skym agent` reads and reports those log lines.
    let report = Report { exceptions: Vec::new(), ..report::build(&c, &host, now) };
    let sent = client.send(outbox::gzip(&report)?).await;
    failed |= match sent {
        Ok((200..=299, _)) => show(Mark::Ok, "report accepted"),
        Ok((401 | 403, _)) => show(Mark::Fail, "report: the server does not accept this token"),
        Ok((status, body)) => {
            show(Mark::Fail, &format!("report: the server answered {status}: {body}"))
        }
        Err(e) => show(Mark::Fail, &format!("report: {e}")),
    };
    Ok(u8::from(failed))
}

/// The outbox can be created and written to, as the user running this.
fn writable(state: &Path) -> anyhow::Result<()> {
    Outbox::open(state)?;
    let probe = state.join("outbox/doctor.tmp");
    std::fs::write(&probe, b"").with_context(|| format!("cannot write {}", probe.display()))?;
    Ok(std::fs::remove_file(&probe)?)
}
