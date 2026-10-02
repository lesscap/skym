//! Folds a workload's log lines into exception groups.

use super::line::{Line, Record, classify};
use super::text::{redact, truncate, unstructured_code};
use super::{Detail, LogLine, Stream};
use jiff::Timestamp;
use skym_core::model::{ExceptionClass, ExceptionGroup, ExceptionSample};
use skym_core::subject::WorkloadKey;
use std::collections::BTreeMap;

const MAX_GROUPS: usize = 100;
const MAX_BIZ_KEYS: usize = 5;

/// `(business?, component, code)`; `ExceptionClass` itself is not ordered.
type GroupKey = (bool, String, String);

pub struct Grouper {
    workload: WorkloadKey,
    detail: Detail,
    groups: BTreeMap<GroupKey, ExceptionGroup>,
    pending: Option<Pending>,
}

/// An unmarked stderr record still collecting its stack.
struct Pending {
    ts: Timestamp,
    record: Record,
    stack: Vec<String>,
    traceback: Traceback,
    /// Started by a logged message (`logger.exception`), which then stays the message.
    logged: bool,
}

/// Where a Python traceback is: frames still coming, the exception line seen, or a
/// "During handling of the above exception…" line announcing a chained traceback.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Traceback {
    No,
    Frames,
    Done,
    Chained,
}

/// What a stderr line does to a Python traceback in progress.
enum Step {
    Start,
    Join,
    Skip,
    Summary,
    Chain,
    Pass,
}

const HEADER: &str = "Traceback (most recent call last)";
const CHAINS: [&str; 2] = [
    "During handling of the above exception, another exception occurred",
    "The above exception was the direct cause of the following exception",
];

impl Grouper {
    pub fn new(workload: WorkloadKey, detail: Detail) -> Self {
        Grouper { workload, detail, groups: BTreeMap::new(), pending: None }
    }

    pub fn push(&mut self, line: LogLine) {
        if line.stream == Stream::Stderr && self.python(&line) {
            return;
        }
        match classify(line.text, line.stream) {
            Line::Marked(r) => self.add(line.ts, r),
            Line::Stderr(r) => {
                self.flush();
                self.pending = Some(Pending {
                    ts: line.ts,
                    record: r,
                    stack: Vec::new(),
                    traceback: Traceback::No,
                    logged: false,
                });
            }
            Line::Continuation => {
                if let Some(p) = &mut self.pending {
                    p.stack.push(line.text.to_string());
                }
            }
            Line::Plain => self.flush(),
            Line::Ignored => {}
        }
    }

    pub fn finish(mut self) -> Vec<ExceptionGroup> {
        self.flush();
        self.groups.into_values().collect()
    }

    /// A Python traceback is one record: an optional logged message, the header, indented
    /// frames, and the exception on the first unindented line, which names the group.
    /// Chained tracebacks continue the same record. Returns whether the line was consumed.
    fn python(&mut self, line: &LogLine) -> bool {
        let text = line.text.trim();
        match self.step(line) {
            Step::Start => {
                self.flush();
                let Line::Stderr(record) = classify(line.text, Stream::Stderr) else {
                    return false;
                };
                self.pending = Some(Pending {
                    ts: line.ts,
                    record,
                    stack: Vec::new(),
                    traceback: Traceback::Frames,
                    logged: false,
                });
            }
            Step::Pass => return false,
            Step::Skip => {}
            step => {
                let Some(p) = self.pending.as_mut() else { return false };
                match step {
                    Step::Summary => {
                        p.record.code = unstructured_code(text);
                        match p.logged {
                            true => p.stack.push(text.to_string()),
                            false => p.record.message = Some(text.to_string()),
                        }
                        p.traceback = Traceback::Done;
                    }
                    Step::Join => {
                        p.logged |= p.traceback == Traceback::No;
                        p.stack.push(text.to_string());
                        p.traceback = Traceback::Frames;
                    }
                    _ => {
                        p.stack.push(text.to_string());
                        p.traceback = Traceback::Chained;
                    }
                }
            }
        }
        true
    }

    fn step(&self, line: &LogLine) -> Step {
        let text = line.text.trim();
        let indented = line.text.starts_with([' ', '\t']);
        let Some(p) = &self.pending else {
            return if text.starts_with(HEADER) { Step::Start } else { Step::Pass };
        };
        let logged = p.traceback == Traceback::No && p.stack.is_empty();
        match p.traceback {
            _ if text.starts_with(HEADER) && (logged || p.traceback == Traceback::Chained) => {
                Step::Join
            }
            _ if text.starts_with(HEADER) => Step::Start,
            Traceback::Frames | Traceback::Done | Traceback::Chained if text.is_empty() => {
                Step::Skip
            }
            Traceback::Frames if !indented => Step::Summary,
            Traceback::Done if CHAINS.iter().any(|c| text.starts_with(c)) => Step::Chain,
            _ => Step::Pass,
        }
    }

    fn flush(&mut self) {
        if let Some(p) = self.pending.take() {
            let stacktrace = (!p.stack.is_empty()).then(|| p.stack.join("\n"));
            self.add(p.ts, Record { stacktrace, ..p.record });
        }
    }

    fn add(&mut self, ts: Timestamp, r: Record) {
        let business = r.class == ExceptionClass::Business;
        let mut key = (business, r.component.clone(), r.code.clone());
        if self.groups.len() >= MAX_GROUPS && !self.groups.contains_key(&key) {
            key = (business, "_skym".into(), "_OVERFLOW".into());
        }
        let sample = self.sample(&r);
        let group = self.groups.entry(key.clone()).or_insert_with(|| ExceptionGroup {
            workload: self.workload.clone(),
            class: r.class,
            component: key.1,
            code: key.2,
            count: 0,
            final_count: 0,
            first_seen: ts,
            last_seen: ts,
            biz_keys: Vec::new(),
            sample: None,
        });
        group.count = group.count.saturating_add(1);
        group.final_count = group.final_count.saturating_add(u32::from(r.is_final));
        group.last_seen = group.last_seen.max(ts);
        group.first_seen = group.first_seen.min(ts);
        if let Some(biz_key) = r.biz_key {
            remember(&mut group.biz_keys, truncate(redact(&biz_key), 128));
        }
        group.sample = sample.or(group.sample.take());
    }

    fn sample(&self, r: &Record) -> Option<ExceptionSample> {
        let keep = self.detail == Detail::Local || r.class != ExceptionClass::Business;
        let limit = |s: &String, max| match self.detail {
            Detail::Report => truncate(redact(s), max),
            Detail::Local => redact(s),
        };
        keep.then(|| ExceptionSample {
            message: r.message.as_ref().map(|m| limit(m, 1000)).unwrap_or_default(),
            exception_type: r.exception_type.clone(),
            stacktrace: r.stacktrace.as_ref().map(|s| limit(s, 8192)),
        })
    }
}

/// Most recent distinct values last, at most `MAX_BIZ_KEYS`.
fn remember(keys: &mut Vec<String>, key: String) {
    keys.retain(|k| *k != key);
    keys.push(key);
    if keys.len() > MAX_BIZ_KEYS {
        keys.remove(0);
    }
}
