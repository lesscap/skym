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
    let err = || SinceError(s.to_string());
    match relative_secs(s) {
        Some(secs) => now
            .checked_sub(SignedDuration::from_secs(secs.ok_or_else(err)?))
            .map_err(|_| err()),
        None => s.parse::<Timestamp>().map_err(|_| err()),
    }
}

/// `None` when `s` is not of the form `<digits><s|m|h|d>`; `Some(None)` on overflow.
fn relative_secs(s: &str) -> Option<Option<i64>> {
    let unit = s.chars().last()?;
    let digits = &s[..s.len() - unit.len_utf8()];
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let factor = match unit {
        's' => 1,
        'm' => 60,
        'h' => 3_600,
        'd' => 86_400,
        _ => return None,
    };
    Some(
        digits
            .parse::<i64>()
            .ok()
            .and_then(|n| n.checked_mul(factor)),
    )
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
    if shown.is_empty() {
        "0s".to_string()
    } else {
        shown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_since_accepts_relative_and_absolute() {
        let now: Timestamp = "2026-10-01T12:00:00Z".parse().unwrap();
        let at = |s: &str| parse_since(s, now).unwrap().to_string();
        assert_eq!(at("15m"), "2026-10-01T11:45:00Z");
        assert_eq!(at("7d"), "2026-09-24T12:00:00Z");
        assert_eq!(at("2026-10-01T10:00:00Z"), "2026-10-01T10:00:00Z");
        assert!(parse_since("6x", now).is_err());
        assert!(parse_since("h", now).is_err());
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
