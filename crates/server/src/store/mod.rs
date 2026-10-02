//! SQLite access. One connection behind a mutex; every call runs on the blocking pool.
//! Query functions take a `&Connection` (a transaction derefs to one).

pub mod history;
pub mod hosts;
pub mod incidents;

use jiff::Timestamp;
use rusqlite::Connection;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct Store {
    conn: Arc<Mutex<Connection>>,
}

impl Store {
    pub fn new(conn: Connection) -> Self {
        Store { conn: Arc::new(Mutex::new(conn)) }
    }

    pub async fn call<T, F>(&self, f: F) -> anyhow::Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> anyhow::Result<T> + Send + 'static,
    {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            // A panic mid-call dropped its transaction, which rolled back: the connection is sound.
            let mut guard = conn.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            f(&mut guard)
        })
        .await?
    }
}

// Stored forms. Timestamps are whole seconds in UTC (`…Z`), so they sort as time.

pub(crate) fn ts(t: Timestamp) -> String {
    Timestamp::from_second(t.as_second()).expect("in range").to_string()
}

pub(crate) fn json<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).expect("serializable")
}

/// A column holding a value's text form (`FromStr`); a bad value is a conversion error.
pub(crate) fn parsed<T>(s: String) -> rusqlite::Result<T>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    s.parse().map_err(|e: T::Err| bad_text(e.to_string()))
}

pub(crate) fn from_json<T: DeserializeOwned>(text: &str) -> rusqlite::Result<T> {
    serde_json::from_str(text).map_err(bad_text)
}

fn bad_text(e: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, e.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_timestamps_sort_as_time() {
        let a: Timestamp = "2026-10-01T12:00:00.900Z".parse().unwrap();
        let b: Timestamp = "2026-10-01T12:00:01Z".parse().unwrap();
        assert_eq!(ts(a), "2026-10-01T12:00:00Z");
        assert!(ts(a) < ts(b));
        assert_eq!(parsed::<Timestamp>(ts(b)).unwrap(), b);
    }
}
