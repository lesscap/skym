//! The latest probe of each endpoint, and since when skym has probed it.

use super::{parsed, ts};
use crate::probe::Probe;
use jiff::Timestamp;
use rusqlite::{Connection, params};

pub struct ProbeRow {
    pub url: String,
    pub first_seen: Timestamp,
    pub probe: Probe,
}

pub fn save(c: &Connection, url: &str, p: &Probe) -> rusqlite::Result<()> {
    let (status, error) = match &p.response {
        Ok(status) => (Some(*status), None),
        Err(why) => (None, Some(why.as_str())),
    };
    c.execute(
        "INSERT INTO probes (url, first_seen, at, status, error, latency_ms, cert_not_after)
         VALUES (?1, ?2, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT (url) DO UPDATE SET at = ?2, status = ?3, error = ?4, latency_ms = ?5,
           cert_not_after = ?6",
        params![url, ts(p.at), status, error, p.latency_ms as i64, p.cert_not_after.map(ts)],
    )?;
    Ok(())
}

pub fn all(c: &Connection) -> rusqlite::Result<Vec<ProbeRow>> {
    let mut stmt = c.prepare(
        "SELECT url, first_seen, at, status, error, latency_ms, cert_not_after FROM probes",
    )?;
    let rows = stmt.query_map([], |r| {
        let status: Option<u16> = r.get(3)?;
        let error: Option<String> = r.get(4)?;
        Ok(ProbeRow {
            url: r.get(0)?,
            first_seen: parsed(r.get(1)?)?,
            probe: Probe {
                at: parsed(r.get(2)?)?,
                response: status.ok_or(error.unwrap_or_default()),
                latency_ms: r.get::<_, i64>(5)? as u64,
                cert_not_after: r.get::<_, Option<String>>(6)?.map(parsed).transpose()?,
            },
        })
    })?;
    rows.collect()
}

/// Forgets endpoints no longer configured.
pub fn prune(c: &Connection, configured: &[&str]) -> rusqlite::Result<()> {
    let urls = serde_json::to_string(configured).expect("serializable");
    c.execute("DELETE FROM probes WHERE url NOT IN (SELECT value FROM json_each(?1))", [urls])?;
    Ok(())
}
