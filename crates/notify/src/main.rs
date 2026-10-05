use jiff::{SignedDuration, Timestamp};
use skym_notify::{Notifier, PASS, PING, config};
use std::path::PathBuf;
use std::time::Duration;
use tokio::signal::unix::{SignalKind, signal};
use tokio::time::{MissedTickBehavior, interval};

const USAGE: &str = "usage: skym-notify [--config <path>]";
/// A lasting failure is logged again this often.
const RELOG: SignedDuration = SignedDuration::from_mins(10);

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let path = match args.as_slice() {
        [] => PathBuf::from("/etc/skym-notify/notify.toml"),
        [flag, path] if flag == "--config" => PathBuf::from(path),
        _ => anyhow::bail!(USAGE),
    };
    let config = config::load(&path)?;
    let mut notifier = Notifier::new(config)?;
    eprintln!("skym-notify {} reading {}", env!("CARGO_PKG_VERSION"), notifier.config.server);

    let tick = |d: SignedDuration| {
        let mut i = interval(Duration::try_from(d).expect("a positive period"));
        i.set_missed_tick_behavior(MissedTickBehavior::Delay);
        i
    };
    let (mut passes, mut pings) = (tick(PASS), tick(PING));
    let (mut term, mut int) = (signal(SignalKind::terminate())?, signal(SignalKind::interrupt())?);
    let (mut pass_log, mut ping_log) = (Throttle::default(), Throttle::default());
    loop {
        tokio::select! {
            _ = passes.tick() => pass_log.note("pass", notifier.pass(Timestamp::now()).await),
            _ = pings.tick() => ping_log.note("ping", notifier.ping(Timestamp::now()).await),
            _ = term.recv() => break,
            _ = int.recv() => break,
        }
    }
    eprintln!("skym-notify stopping");
    Ok(())
}

/// The first failure, then one line per [`RELOG`] while it lasts, then the recovery.
#[derive(Default)]
struct Throttle {
    logged_at: Option<Timestamp>,
}

impl Throttle {
    fn note(&mut self, what: &str, result: anyhow::Result<()>) {
        let now = Timestamp::now();
        match result {
            Ok(()) if self.logged_at.take().is_some() => eprintln!("{what}: recovered"),
            Ok(()) => {}
            Err(e) if self.logged_at.is_none_or(|t| now.duration_since(t) >= RELOG) => {
                eprintln!("{what}: {e:#}");
                self.logged_at = Some(now);
            }
            Err(_) => {}
        }
    }
}
