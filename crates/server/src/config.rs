//! The server configuration (TOML). Tokens are stored only as SHA-256 hashes.

use anyhow::{Context, bail};
use jiff::{SignedDuration, Timestamp};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use skym_core::rules::IncidentCode;
use skym_core::subject::{CustomerId, HostId, Subject};
use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

#[derive(Deserialize, Debug, Clone)]
#[serde(default)]
pub struct ServerConfig {
    pub listen: SocketAddr,
    pub database: PathBuf,
    /// Must match the agents' interval: heartbeat timeout is three of them.
    pub report_interval: SignedDuration,
    pub customers: Vec<Customer>,
    pub hosts: Vec<HostEntry>,
    pub readers: Vec<Reader>,
    pub mute: Vec<Mute>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        ServerConfig {
            listen: ([127, 0, 0, 1], 7280).into(),
            database: "/var/lib/skym-server/skym.db".into(),
            report_interval: SignedDuration::from_secs(60),
            customers: Vec::new(),
            hosts: Vec::new(),
            readers: Vec::new(),
            mute: Vec::new(),
        }
    }
}

#[derive(Deserialize, Debug, Clone)]
pub struct Customer {
    pub id: CustomerId,
    pub name: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct HostEntry {
    pub id: HostId,
    pub customer: CustomerId,
    pub token_sha256: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Reader {
    pub name: String,
    pub token_sha256: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Mute {
    pub subject: Subject,
    pub code: IncidentCode,
    pub reason: Option<String>,
    pub until: Option<Timestamp>,
}

pub fn load(path: &Path) -> anyhow::Result<ServerConfig> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    parse(&text).with_context(|| format!("invalid {}", path.display()))
}

pub fn parse(text: &str) -> anyhow::Result<ServerConfig> {
    let cfg: ServerConfig = toml::from_str(text)?;
    validate(&cfg)?;
    Ok(cfg)
}

fn validate(cfg: &ServerConfig) -> anyhow::Result<()> {
    let customers: BTreeSet<&str> = cfg.customers.iter().map(|c| c.id.as_str()).collect();
    let mut ids = BTreeSet::new();
    for h in &cfg.hosts {
        if h.id.is_empty() || h.id.contains(['/', ':']) || !ids.insert(&h.id) {
            bail!("host id {:?} is empty, contains / or :, or is repeated", h.id);
        }
        if !customers.contains(h.customer.as_str()) {
            bail!("host {:?} names unknown customer {:?}", h.id, h.customer);
        }
    }
    let hashes = cfg
        .hosts
        .iter()
        .map(|h| &h.token_sha256)
        .chain(cfg.readers.iter().map(|r| &r.token_sha256));
    let mut seen = BTreeSet::new();
    for hash in hashes {
        if hash.len() != 64 || !hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            bail!("token_sha256 must be 64 lowercase hex characters");
        }
        if !seen.insert(hash) {
            bail!("a token hash is used twice; every host and reader needs its own token");
        }
    }
    Ok(())
}

pub fn sha256_hex(s: &str) -> String {
    Sha256::digest(s.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// A new random token (64 hex characters) and the hash to put in the configuration.
pub fn new_token() -> anyhow::Result<(String, String)> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("no randomness: {e}"))?;
    let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let hash = sha256_hex(&token);
    Ok((token, hash))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(c: char) -> String {
        c.to_string().repeat(64)
    }

    fn config(hosts: &str) -> String {
        format!("[[customers]]\nid = \"acme\"\nname = \"Acme\"\n{hosts}")
    }

    fn host(id: &str, customer: &str, h: &str) -> String {
        format!("[[hosts]]\nid = \"{id}\"\ncustomer = \"{customer}\"\ntoken_sha256 = \"{h}\"\n")
    }

    #[test]
    fn valid_configuration_parses_with_defaults() {
        let reader = format!("[[readers]]\nname = \"ops\"\ntoken_sha256 = \"{}\"\n", hash('b'));
        let mute =
            "[[mute]]\nsubject = \"workload:x/legacy/worker\"\ncode = \"WORKLOAD_UNHEALTHY\"\n";
        let cfg = parse(&config(&(host("x", "acme", &hash('a')) + &reader + mute))).unwrap();
        assert_eq!(cfg.report_interval, SignedDuration::from_secs(60));
        assert_eq!((cfg.hosts.len(), cfg.readers.len(), cfg.mute.len()), (1, 1, 1));
    }

    #[test]
    fn invalid_configurations_are_rejected() {
        let reader_same =
            format!("[[readers]]\nname = \"ops\"\ntoken_sha256 = \"{}\"\n", hash('a'));
        let cases = [
            config(&(host("x", "acme", &hash('a')) + &host("x", "acme", &hash('b')))),
            config(&host("a/b", "acme", &hash('a'))),
            config(&host("x", "nobody", &hash('a'))),
            config(&host("x", "acme", "short")),
            config(&host("x", "acme", &hash('A'))),
            config(&(host("x", "acme", &hash('a')) + &reader_same)),
            config("[[mute]]\nsubject = \"nonsense\"\ncode = \"WORKLOAD_DOWN\"\n"),
            config("[[mute]]\nsubject = \"host:x\"\ncode = \"NOT_A_CODE\"\n"),
        ];
        for (i, text) in cases.iter().enumerate() {
            assert!(parse(text).is_err(), "case {i} should be rejected");
        }
    }

    #[test]
    fn load_reads_and_validates_a_file() {
        let path =
            std::env::temp_dir().join(format!("skym-server-test-{}.toml", std::process::id()));
        std::fs::write(&path, config(&host("x", "acme", &hash('a')))).unwrap();
        assert_eq!(load(&path).unwrap().hosts[0].id, "x");
        std::fs::write(&path, config(&host("x", "nobody", &hash('a')))).unwrap();
        assert!(load(&path).is_err());
        std::fs::remove_file(&path).unwrap();
        assert!(load(&path).is_err(), "a missing file is an error");
    }

    #[test]
    fn tokens_are_random_and_hash_to_their_entry() {
        let (a, ha) = new_token().unwrap();
        let (b, _) = new_token().unwrap();
        assert_ne!(a, b);
        assert_eq!((a.len(), sha256_hex(&a)), (64, ha));
    }
}
