//! SQLite access. One connection behind a mutex; every call runs on the blocking pool.
//! Query functions take a `&Connection` (a transaction derefs to one).

pub mod history;
pub mod hosts;
pub mod incidents;

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

pub(crate) fn json<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).expect("serializable")
}

/// A column holding a value's text form (`FromStr`); a bad value is a conversion error.
pub(crate) fn parsed<T>(s: String) -> rusqlite::Result<T>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    s.parse().map_err(|e: T::Err| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            e.to_string().into(),
        )
    })
}

pub(crate) fn from_json<T: DeserializeOwned>(text: &str) -> rusqlite::Result<T> {
    serde_json::from_str(text).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })
}
