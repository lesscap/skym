//! Container logs → exception groups.

use super::split;
use crate::exceptions::{Detail, Grouper, LogLine, Stream};
use bollard::Docker;
use bollard::container::LogOutput;
use bollard::query_parameters::LogsOptions;
use futures_util::{StreamExt, stream};
use jiff::Timestamp;
use skym_core::model::ExceptionGroup;
use skym_core::subject::WorkloadKey;
use std::collections::BTreeMap;
use std::time::Duration;
use tokio::time::timeout;

/// Docker can stall on either way of reading: `tail` on some json-file logs (Docker 24),
/// `since` alone on large logs (it scans from the start). Each attempt gets a deadline.
const ATTEMPTS: [&str; 2] = ["20000", "all"];
const DEADLINE: Duration = Duration::from_secs(3);

/// One workload's groups, and the time of its last line read.
type OneRead = (Vec<ExceptionGroup>, Option<(WorkloadKey, Timestamp)>);

/// What one pass read from the logs.
#[derive(Default)]
pub struct Read {
    pub groups: Vec<ExceptionGroup>,
    /// The time of the last line read, per workload that had any.
    pub ends: BTreeMap<WorkloadKey, Timestamp>,
    pub errors: Vec<String>,
}

/// Each target's lines strictly after its own time.
pub async fn read(
    docker: &Docker,
    targets: &[(WorkloadKey, String, Timestamp)],
    detail: Detail,
) -> Read {
    let results: Vec<Result<OneRead, String>> = stream::iter(targets)
        .map(|(key, id, after)| read_one(docker, key, id, *after, detail))
        .buffer_unordered(8)
        .collect()
        .await;
    let (read, errors) = split(results);
    let (groups, ends): (Vec<_>, Vec<_>) = read.into_iter().unzip();
    Read { groups: groups.concat(), ends: ends.into_iter().flatten().collect(), errors }
}

async fn read_one(
    docker: &Docker,
    key: &WorkloadKey,
    id: &str,
    after: Timestamp,
    detail: Detail,
) -> Result<OneRead, String> {
    let label = format!("logs {}/{}", key.project, key.service);
    for tail in ATTEMPTS {
        if let Ok(result) = timeout(DEADLINE, read_tail(docker, key, id, after, detail, tail)).await
        {
            return result.map_err(|e| format!("{label}: {e}"));
        }
    }
    Err(format!("{label}: Docker did not answer within {}s", DEADLINE.as_secs()))
}

async fn read_tail(
    docker: &Docker,
    key: &WorkloadKey,
    id: &str,
    after: Timestamp,
    detail: Detail,
    tail: &str,
) -> Result<OneRead, bollard::errors::Error> {
    let mut frames = docker.logs(id, Some(options(after, tail)));
    let mut grouper = Grouper::new(key.clone(), detail);
    let (mut out, mut err) = (LineAssembler::default(), LineAssembler::default());
    let mut last: Option<Timestamp> = None;
    // Docker's `since` has whole seconds: lines are kept by their own time instead.
    let mut feed = |stream, lines: Vec<(Timestamp, String)>| {
        for (ts, text) in lines.iter().filter(|(ts, _)| *ts > after) {
            last = last.max(Some(*ts));
            grouper.push(LogLine { ts: *ts, stream, text });
        }
    };
    while let Some(frame) = frames.next().await {
        match frame? {
            LogOutput::StdErr { message } => {
                feed(Stream::Stderr, err.push(&String::from_utf8_lossy(&message)))
            }
            LogOutput::StdOut { message } | LogOutput::Console { message } => {
                feed(Stream::Stdout, out.push(&String::from_utf8_lossy(&message)))
            }
            LogOutput::StdIn { .. } => {}
        }
    }
    // A line without its newline yet is still being written: it is read whole next time.
    Ok((grouper.finish(), last.map(|t| (key.clone(), t))))
}

/// Both streams, each line prefixed with its timestamp (needed to reassemble lines).
fn options(since: Timestamp, tail: &str) -> LogsOptions {
    LogsOptions {
        stdout: true,
        stderr: true,
        timestamps: true,
        since: since.as_second() as i32,
        tail: tail.into(),
        ..Default::default()
    }
}

/// Rebuilds log lines from Docker's frames. Every entry starts with a timestamp, and a line
/// longer than 16 KB (or TTY output) arrives as several entries; a line ends at `\n`.
#[derive(Default)]
pub struct LineAssembler {
    pending: Option<(Timestamp, String)>,
    last: Option<Timestamp>,
}

impl LineAssembler {
    /// The lines completed by this frame, each stamped with its first fragment's time.
    pub fn push(&mut self, frame: &str) -> Vec<(Timestamp, String)> {
        frame
            .split_inclusive('\n')
            .filter_map(|segment| {
                let complete = segment.ends_with('\n');
                let segment = segment.trim_end_matches(['\n', '\r']);
                let (ts, text) = match (split_ts(segment), self.last) {
                    (Some(stamped), _) => stamped,
                    // More lines inside one TTY entry. Known limit: such a line that itself
                    // starts with an RFC 3339 timestamp is taken for Docker's prefix.
                    (None, Some(last)) => (last, segment),
                    (None, None) => return None,
                };
                self.last = Some(ts);
                match &mut self.pending {
                    Some((_, line)) => line.push_str(text),
                    None => self.pending = Some((ts, text.to_string())),
                }
                complete.then(|| self.pending.take()).flatten()
            })
            .collect()
    }
}

/// `"2026-10-01T12:00:00.123456789Z message"` → timestamp and message.
pub fn split_ts(line: &str) -> Option<(Timestamp, &str)> {
    let (ts, text) = line.split_once(' ').unwrap_or((line, ""));
    Some((ts.parse().ok()?, text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_docker_timestamps() {
        let (ts, text) = split_ts("2026-10-01T12:00:00.123456789Z  at foo").unwrap();
        assert_eq!((ts.as_second(), text), (1_790_856_000, " at foo"));
        assert_eq!(split_ts("2026-10-01T12:00:00Z").unwrap().1, "");
        assert!(split_ts("no timestamp").is_none());
    }

    #[test]
    fn asks_docker_for_both_streams_with_timestamps() {
        let o = options(Timestamp::from_second(1_790_000_000).unwrap(), "20000");
        assert!(o.stdout && o.stderr && o.timestamps && !o.follow);
        assert_eq!((o.since, o.tail.as_str()), (1_790_000_000, "20000"));
    }

    #[test]
    fn assembles_lines_across_frames() {
        let t = |s: u8| format!("2026-10-01T12:00:{s:02}Z");
        let mut a = LineAssembler::default();
        // two complete lines in one frame
        let two = a.push(&format!("{} first\r\n{} second\n", t(1), t(2)));
        assert_eq!(two.iter().map(|(_, l)| l.as_str()).collect::<Vec<_>>(), ["first", "second"]);
        // a long line split by Docker into partial entries, each with its own timestamp
        assert!(a.push(&format!("{} {{\"skym\":\"exc", t(3))).is_empty());
        assert!(a.push(&format!("{} eption\",", t(4))).is_empty());
        let joined = a.push(&format!("{} \"code\":\"X\"}}\n", t(5)));
        assert_eq!(joined.len(), 1);
        assert_eq!(joined[0].1, r#"{"skym":"exception","code":"X"}"#);
        assert_eq!(joined[0].0.as_second() % 60, 3, "stamped with the first fragment");
        // a TTY entry holding several lines, then a trailing half line
        let tty = a.push(&format!("{} one\ntwo\nthr", t(6)));
        assert_eq!(tty.iter().map(|(_, l)| l.as_str()).collect::<Vec<_>>(), ["one", "two"]);
    }
}
