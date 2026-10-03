//! `skym doctor`: is this installation ready to report? Ends by sending one real report.

use crate::collect::{self, Collected};
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

/// Prints one check; whether it failed.
fn show(mark: Mark, what: &str) -> bool {
    let sign = match mark {
        Mark::Ok => "✓",
        Mark::Warn => "⚠",
        Mark::Fail => "✗",
    };
    println!("{sign} {what}");
    matches!(mark, Mark::Fail)
}

/// Exit code 1 when any check fails. Without a server, a token or a reachable server
/// the later checks cannot run.
pub async fn run(cfg: &Config) -> anyhow::Result<u8> {
    let Some(server) = cfg.server.as_deref() else {
        show(Mark::Fail, "server: not set in the configuration");
        return Ok(1);
    };
    show(Mark::Ok, &format!("server: {server}"));
    let Some(token) = token(&cfg.token_file) else { return Ok(1) };
    let mut failed = state(&cfg.state_dir);
    let now = Timestamp::now();
    let (host, c) = collect::once(cfg, now, now - cfg.interval, Detail::Report).await;
    failed |= collection(&c);
    let client = Client::new(server, token)?;
    if !reachable(&client).await {
        return Ok(1);
    }
    // Without exception groups: a running `skym agent` reads and reports those log lines.
    let report = Report { exceptions: Vec::new(), ..report::build(&c, &host, now) };
    failed |= rejected(&client, &report).await?;
    Ok(u8::from(failed))
}

/// The token, if readable; warns when others can read it too.
fn token(path: &Path) -> Option<String> {
    let token = deliver::read_token(path)
        .and_then(|t| Ok((t, std::fs::metadata(path)?.permissions().mode())));
    match token {
        Ok((token, mode)) if mode & 0o077 == 0 => {
            show(Mark::Ok, &format!("token: {}", path.display()));
            Some(token)
        }
        Ok((token, _)) => {
            let what = format!("token: {} is readable by others; chmod 600 it", path.display());
            show(Mark::Warn, &what);
            Some(token)
        }
        Err(e) => {
            show(Mark::Fail, &format!("token: {e:#}"));
            None
        }
    }
}

fn state(dir: &Path) -> bool {
    match writable(dir) {
        Ok(()) => show(Mark::Ok, &format!("state: {} is writable", dir.display())),
        Err(e) => show(Mark::Fail, &format!("state: {e:#}")),
    }
}

/// The outbox can be created and written to, as the user running this.
fn writable(dir: &Path) -> anyhow::Result<()> {
    Outbox::open(dir)?;
    let probe = dir.join("outbox/doctor.tmp");
    std::fs::write(&probe, b"").with_context(|| format!("cannot write {}", probe.display()))?;
    Ok(std::fs::remove_file(&probe)?)
}

fn collection(c: &Collected) -> bool {
    let collected = format!("collection: {} workloads", c.workloads.len());
    match (c.all_failed, c.errors.len()) {
        (true, _) => show(Mark::Fail, "collection: every source failed (errors above)"),
        (false, 0) => show(Mark::Ok, &collected),
        (false, n) => show(Mark::Warn, &format!("{collected}; {n} sources failed (errors above)")),
    }
}

async fn reachable(client: &Client) -> bool {
    let failed = match client.healthz().await {
        Ok((200, _)) => show(Mark::Ok, "server reachable"),
        Ok((status, _)) => show(Mark::Fail, &format!("server answered /healthz with {status}")),
        Err(e) => show(Mark::Fail, &format!("server unreachable: {e}")),
    };
    !failed
}

async fn rejected(client: &Client, report: &Report) -> anyhow::Result<bool> {
    Ok(match client.send(outbox::gzip(report)?).await {
        Ok((200..=299, _)) => show(Mark::Ok, "report accepted"),
        Ok((401 | 403, _)) => show(Mark::Fail, "report: the server does not accept this token"),
        Ok((status, body)) => {
            show(Mark::Fail, &format!("report: the server answered {status}: {body}"))
        }
        Err(e) => show(Mark::Fail, &format!("report: {e}")),
    })
}
