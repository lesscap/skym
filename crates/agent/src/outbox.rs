//! Reports waiting for delivery: one gzip file per report in `state_dir/outbox`, named by
//! the report's time so that name order is time order. Sent oldest first.

use anyhow::Context;
use flate2::Compression;
use flate2::write::GzEncoder;
use jiff::{SignedDuration, Timestamp};
use skym_core::report::Report;
use std::io::Write;
use std::path::{Path, PathBuf};

/// A day of reports at the default interval.
const MAX_FILES: usize = 1440;
const MAX_AGE: SignedDuration = SignedDuration::from_hours(24);

pub struct Outbox {
    dir: PathBuf,
}

impl Outbox {
    /// Creates the directory, and removes reports a crash left half written.
    pub fn open(state_dir: &Path) -> anyhow::Result<Self> {
        let dir = state_dir.join("outbox");
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("cannot create {}", dir.display()))?;
        for entry in std::fs::read_dir(&dir)? {
            let path = entry?.path();
            if path.extension().is_some_and(|e| e == "tmp") {
                std::fs::remove_file(&path)
                    .with_context(|| format!("cannot remove {}", path.display()))?;
            }
        }
        Ok(Outbox { dir })
    }

    /// Stored atomically: a crash leaves either the whole report or none.
    pub fn push(&self, r: &Report) -> anyhow::Result<()> {
        let path = self.dir.join(name(r.ts));
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, gzip(r)?)
            .with_context(|| format!("cannot write {}", tmp.display()))?;
        std::fs::rename(&tmp, &path).with_context(|| format!("cannot store {}", path.display()))
    }

    /// Stored reports, oldest first.
    pub fn pending(&self) -> anyhow::Result<Vec<PathBuf>> {
        let mut files: Vec<PathBuf> =
            std::fs::read_dir(&self.dir)?.map(|e| e.map(|e| e.path())).collect::<Result<_, _>>()?;
        files.retain(|p| p.to_string_lossy().ends_with(".json.gz"));
        files.sort();
        Ok(files)
    }

    /// Drops reports older than a day and, beyond 1440, the oldest. Returns how many.
    pub fn prune(&self, now: Timestamp) -> anyhow::Result<usize> {
        let doomed = expired(&self.pending()?, now);
        for path in &doomed {
            std::fs::remove_file(path)
                .with_context(|| format!("cannot remove {}", path.display()))?;
        }
        Ok(doomed.len())
    }
}

/// The report as it is sent: gzip-compressed JSON.
pub fn gzip(r: &Report) -> anyhow::Result<Vec<u8>> {
    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    gz.write_all(&serde_json::to_vec(r)?)?;
    Ok(gz.finish()?)
}

/// `<unix nanoseconds, 20 digits>.json.gz`
fn name(ts: Timestamp) -> String {
    format!("{:020}.json.gz", ts.as_nanosecond())
}

fn time_of(path: &Path) -> Option<Timestamp> {
    let stem = path.file_name()?.to_str()?.strip_suffix(".json.gz")?;
    Timestamp::from_nanosecond(stem.parse().ok()?).ok()
}

/// Files to drop, given all of them oldest first. A name that is not a time is kept.
fn expired(oldest_first: &[PathBuf], now: Timestamp) -> Vec<PathBuf> {
    let excess = oldest_first.len().saturating_sub(MAX_FILES);
    oldest_first
        .iter()
        .enumerate()
        .filter(|(i, p)| *i < excess || time_of(p).is_some_and(|t| now.duration_since(t) > MAX_AGE))
        .map(|(_, p)| p.clone())
        .collect()
}

#[cfg(test)]
#[path = "outbox_tests.rs"]
mod tests;
