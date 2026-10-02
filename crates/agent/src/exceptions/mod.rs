//! The exception protocol (docs/exception-protocol.md): one log line at a time in,
//! grouped exceptions out. Pure; no I/O.

mod group;
mod line;
#[cfg(test)]
mod tests;
mod text;
mod traceback;

pub use group::Grouper;

use jiff::Timestamp;

/// `Report`: bounded and redacted for leaving the host. `Local`: full detail for `skym exceptions`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Detail {
    Report,
    Local,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stream {
    Stdout,
    Stderr,
}

#[derive(Clone, Copy, Debug)]
pub struct LogLine<'a> {
    pub ts: Timestamp,
    pub stream: Stream,
    pub text: &'a str,
}
