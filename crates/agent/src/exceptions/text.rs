use regex::Regex;
use sha2::{Digest, Sha256};
use std::sync::LazyLock;

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid regex")
}

/// Replacements applied in order before hashing, so one bug produces one fingerprint.
static NORMALIZE: LazyLock<[(Regex, &str); 4]> = LazyLock::new(|| {
    [
        (re(r#""[^"]*"|'[^']*'"#), "_"),
        (re(r"(?i)\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b"), "_"),
        (re(r"(?i)0x[0-9a-f]+|\b[0-9a-f]{8,}\b"), "_"),
        (re(r"\d+"), "0"),
    ]
});

static REDACT: LazyLock<[(Regex, &str); 6]> = LazyLock::new(|| {
    [
        (
            re(r"(?s)-----BEGIN [A-Z ]*PRIVATE KEY-----.*?-----END [A-Z ]*PRIVATE KEY-----"),
            "[REDACTED]",
        ),
        (re(r"\beyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+"), "[REDACTED]"),
        (re(r"(?i)\bbearer\s+\S+"), "Bearer [REDACTED]"),
        (re(r"(?i)\bauthorization\s*[:=]\s*basic\s+\S+"), "Authorization: Basic [REDACTED]"),
        (re(r"://[^/\s:@]+:[^/\s@]+@"), "://[REDACTED]@"),
        (
            re(
                r#"(?i)\b([A-Za-z0-9_]*?(?:password|passwd|pwd|secret(?:[_-]?key)?|token|api[_-]?key|access[_-]?key))(["']?\s*[:=]\s*)("[^"]*"|'[^']*'|[^\s"'&,;]+)"#,
            ),
            "$1$2[REDACTED]",
        ),
    ]
});

/// `Error`, `ValueError:`, `panic`, `Traceback`... but not `0 errors`.
static ERRORISH: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)(error|exception|panic|panicked|fatal|traceback|unhandled)\b"));

/// An explicit lower level: logfmt or JSON `level`, or a level word near the start
/// (`2026-10-01 12:00:00,123 INFO …`, `WARNING:root:…`).
static QUIET: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r#"(?i:\blevel"?\s*[=:]\s*"?(?:warn|warning|info|debug|trace)\b)|^.{0,40}?\b(?:INFO|DEBUG|TRACE|WARN|WARNING)\b"#,
    )
});

pub fn looks_like_error(line: &str) -> bool {
    ERRORISH.is_match(line) && !QUIET.is_match(line)
}

/// The group code of an unmarked stderr record.
pub fn unstructured_code(line: &str) -> String {
    format!("_UNSTRUCTURED:{}", fingerprint(line))
}

/// 8 hex characters identifying a line up to its variable parts.
pub fn fingerprint(line: &str) -> String {
    let normalized = NORMALIZE
        .iter()
        .fold(line.trim().to_string(), |s, (r, with)| r.replace_all(&s, *with).into_owned());
    Sha256::digest(normalized)[..4].iter().map(|b| format!("{b:02x}")).collect()
}

pub fn redact(s: &str) -> String {
    REDACT.iter().fold(s.to_string(), |s, (r, with)| r.replace_all(&s, *with).into_owned())
}

/// At most `max` characters.
pub fn truncate(s: String, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((end, _)) => s[..end].to_string(),
        None => s,
    }
}
