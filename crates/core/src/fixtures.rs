//! The full sample report, `tests/fixtures/report-full.json`, for other crates' tests.

use crate::report::Report;

pub fn full_report() -> Report {
    serde_json::from_str(include_str!("../tests/fixtures/report-full.json")).expect("valid fixture")
}
