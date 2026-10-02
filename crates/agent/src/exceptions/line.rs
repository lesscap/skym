//! What one log line means under the exception protocol.

use super::Stream;
use super::text::{looks_like_error, unstructured_code};
use regex::Regex;
use serde::Deserialize;
use skym_core::model::ExceptionClass;
use std::sync::LazyLock;

/// One failure, ready to be grouped.
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    pub class: ExceptionClass,
    pub component: String,
    pub code: String,
    pub message: Option<String>,
    pub biz_key: Option<String>,
    pub is_final: bool,
    pub exception_type: Option<String>,
    pub stacktrace: Option<String>,
}

#[derive(Debug, PartialEq)]
pub enum Line {
    /// A protocol line (or a protocol error): complete on its own.
    Marked(Record),
    /// An unmarked stderr line that looks like an error; indented lines may follow.
    Stderr(Record),
    /// An indented stderr line: part of the preceding stderr record, if any.
    Continuation,
    /// Any other stderr line: ends the preceding stderr record.
    Plain,
    /// Stdout without a marker, or a marker of a kind this version does not know.
    Ignored,
}

#[derive(Deserialize)]
struct Marked {
    skym: Option<String>,
    class: Option<String>,
    component: Option<String>,
    code: Option<String>,
    message: Option<String>,
    msg: Option<String>,
    biz_key: Option<String>,
    #[serde(rename = "final")]
    is_final: Option<bool>,
    #[serde(rename = "exception.type")]
    exception_type: Option<String>,
    #[serde(rename = "exception.message")]
    exception_message: Option<String>,
    #[serde(rename = "exception.stacktrace")]
    exception_stacktrace: Option<String>,
}

static COMPONENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[a-z0-9_.-]{1,64}$").unwrap());
static CODE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Z][A-Z0-9_]{0,63}$").unwrap());

pub fn classify(text: &str, stream: Stream) -> Line {
    let trimmed = text.trim_start();
    if trimmed.starts_with('{') && text.contains("\"skym\"") {
        return match serde_json::from_str::<Marked>(trimmed) {
            Err(_) if serde_json::from_str::<serde_json::Value>(trimmed).is_err() => {
                Line::Marked(protocol_error("not valid JSON"))
            }
            Err(_) => Line::Marked(protocol_error(
                "wrong field type or repeated field: final is a boolean, the others are strings",
            )),
            Ok(m) => match m.skym.as_deref() {
                Some("exception") => Line::Marked(record(m).unwrap_or_else(protocol_error)),
                Some(_) => Line::Ignored,
                None => unmarked(text, stream),
            },
        };
    }
    unmarked(text, stream)
}

fn unmarked(text: &str, stream: Stream) -> Line {
    match stream {
        Stream::Stdout => Line::Ignored,
        Stream::Stderr if text.starts_with([' ', '\t']) && !text.trim().is_empty() => {
            Line::Continuation
        }
        Stream::Stderr if looks_like_error(text) => Line::Stderr(Record {
            class: ExceptionClass::Application,
            component: "_stderr".into(),
            code: unstructured_code(text),
            message: Some(text.trim().to_string()),
            biz_key: None,
            is_final: true,
            exception_type: None,
            stacktrace: None,
        }),
        Stream::Stderr => Line::Plain,
    }
}

/// A protocol line, or why it is not one (fixed wording: line content never leaves the host).
fn record(m: Marked) -> Result<Record, &'static str> {
    let class = match m.class.as_deref() {
        Some("application") => ExceptionClass::Application,
        Some("business") => ExceptionClass::Business,
        _ => return Err("class must be application or business"),
    };
    let component = m
        .component
        .filter(|c| COMPONENT.is_match(c))
        .ok_or("component must match [a-z0-9_.-], at most 64 characters")?;
    let code = m
        .code
        .filter(|c| CODE.is_match(c))
        .ok_or("code must be UPPER_SNAKE, at most 64 characters")?;
    Ok(Record {
        class,
        component,
        code,
        message: m.message.or(m.msg).or(m.exception_message),
        biz_key: m.biz_key,
        is_final: m.is_final.unwrap_or(true),
        exception_type: m.exception_type,
        stacktrace: m.exception_stacktrace,
    })
}

fn protocol_error(reason: &str) -> Record {
    Record {
        class: ExceptionClass::Application,
        component: "_skym".into(),
        code: "_PROTOCOL_ERROR".into(),
        message: Some(reason.to_string()),
        biz_key: None,
        is_final: true,
        exception_type: None,
        stacktrace: None,
    }
}
