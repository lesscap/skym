pub use jiff::{SignedDuration, Timestamp};

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SinceError(pub String);

impl fmt::Display for SinceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid since value: {}", self.0)
    }
}

impl std::error::Error for SinceError {}

/// Parses a relative duration (`15m`, `6h`, `7d`) back from `now`, or an RFC 3339 timestamp.
pub fn parse_since(s: &str, now: Timestamp) -> Result<Timestamp, SinceError> {
    let since = match relative(s) {
        Some((n, unit_secs)) => n
            .checked_mul(unit_secs)
            .and_then(|secs| now.checked_sub(SignedDuration::from_secs(secs)).ok()),
        None => s.parse().ok(),
    };
    since.ok_or_else(|| SinceError(s.to_string()))
}

/// `"15m"` → `(15, 60)`: the count and the seconds per unit.
fn relative(s: &str) -> Option<(i64, i64)> {
    let unit_secs = match s.bytes().last()? {
        b's' => 1,
        b'm' => 60,
        b'h' => 3_600,
        b'd' => 86_400,
        _ => return None,
    };
    let digits = &s[..s.len() - 1];
    let all_digits = !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit());
    all_digits.then(|| digits.parse().ok().map(|n| (n, unit_secs)))?
}

/// Formats a duration with its two largest units, e.g. `3h12m`, `2d3h`, `40s`.
pub fn format_duration(d: SignedDuration) -> String {
    let total = d.as_secs().max(0);
    let parts = [
        (total / 86_400, "d"),
        (total % 86_400 / 3_600, "h"),
        (total % 3_600 / 60, "m"),
        (total % 60, "s"),
    ];
    let shown: String = parts
        .iter()
        .skip_while(|(n, _)| *n == 0)
        .take(2)
        .filter(|(n, _)| *n > 0)
        .map(|(n, unit)| format!("{n}{unit}"))
        .collect();
    if shown.is_empty() { "0s".to_string() } else { shown }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_since_accepts_relative_and_absolute() {
        let now: Timestamp = "2026-10-01T12:00:00Z".parse().unwrap();
        let at = |s: &str| parse_since(s, now).unwrap().to_string();
        assert_eq!(at("30s"), "2026-10-01T11:59:30Z");
        assert_eq!(at("15m"), "2026-10-01T11:45:00Z");
        assert_eq!(at("6h"), "2026-10-01T06:00:00Z");
        assert_eq!(at("7d"), "2026-09-24T12:00:00Z");
        assert_eq!(at("2026-10-01T10:00:00Z"), "2026-10-01T10:00:00Z");
        assert!(parse_since("6x", now).is_err());
        assert!(parse_since("h", now).is_err());
        assert!(parse_since("-5m", now).is_err());
        assert!(parse_since("+5m", now).is_err());
        assert!(parse_since("99999999999999999999d", now).is_err());
    }

    #[test]
    fn format_duration_shows_two_largest_units() {
        let f = |secs| format_duration(SignedDuration::from_secs(secs));
        assert_eq!(f(3 * 3_600 + 12 * 60 + 5), "3h12m");
        assert_eq!(f(2 * 86_400 + 3 * 3_600), "2d3h");
        assert_eq!(f(86_400 + 30), "1d");
        assert_eq!(f(40), "40s");
        assert_eq!(f(0), "0s");
        assert_eq!(f(-5), "0s");
    }
}
