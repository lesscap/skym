//! What has been announced, kept in a small JSON file between passes and restarts.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use skym_core::rules::{IncidentCode, Severity};
use skym_core::subject::Subject;
use std::path::Path;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct State {
    pub announced: Vec<Announced>,
}

/// One incident, as long as it was announced and not yet announced resolved.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Announced {
    pub id: Id,
    /// The worst severity announced: a later "worse" is said once.
    pub severity: Severity,
    pub label: Label,
    /// Since when it has been missing from the open incidents.
    #[serde(default)]
    pub absent_since: Option<Timestamp>,
}

/// An incident across its life: reopening within half an hour keeps the same one.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Id {
    pub subject: Subject,
    pub code: IncidentCode,
    pub opened_at: Option<Timestamp>,
}

/// How a message names it: the host, then its application or what it is about.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Label {
    pub host: String,
    pub what: String,
}

/// A missing file is a first run. One that cannot be read starts over: announcing again
/// beats announcing nothing.
pub fn load(path: &Path) -> State {
    match std::fs::read_to_string(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => State::default(),
        Err(e) => {
            eprintln!("warning: cannot read {}: {e}; starting over", path.display());
            State::default()
        }
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
            eprintln!("warning: {} is not a state file ({e}); starting over", path.display());
            State::default()
        }),
    }
}

/// Written whole and renamed into place, so a crash never leaves half a file.
pub fn save(path: &Path, state: &State) -> anyhow::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(state)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_round_trips_and_a_missing_or_bad_file_starts_empty() {
        let dir = std::env::temp_dir().join(format!("skym-notify-state-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.json");
        assert_eq!(load(&path), State::default(), "a first run");
        let state = State {
            announced: vec![Announced {
                id: Id {
                    subject: "workload:x/shop/api".parse().unwrap(),
                    code: IncidentCode::WorkloadDown,
                    opened_at: Some(Timestamp::from_second(1_790_000_000).unwrap()),
                },
                severity: Severity::Critical,
                label: Label { host: "x".into(), what: "shop".into() },
                absent_since: None,
            }],
        };
        save(&path, &state).unwrap();
        assert_eq!(load(&path), state);
        assert!(!path.with_extension("tmp").exists(), "renamed into place");
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(load(&path), State::default());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
