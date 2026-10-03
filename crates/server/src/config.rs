//! The server configuration (TOML). Tokens are stored only as SHA-256 hashes.

use anyhow::{Context, bail};
use jiff::{SignedDuration, Timestamp};
use reqwest::header::{HeaderName, HeaderValue};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use skym_core::rules::IncidentCode;
use skym_core::subject::{AppKey, CustomerId, HostId, Subject};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
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
    pub endpoints: Vec<Endpoint>,
    pub apps: Vec<AppConfig>,
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
            endpoints: Vec::new(),
            apps: Vec::new(),
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

/// A URL the server probes. `expect` replaces the default "below 400" when set; `headers`
/// may hold a token for the probe, so their values never leave the configuration.
#[derive(Deserialize, Debug, Clone)]
pub struct Endpoint {
    pub url: String,
    pub customer: CustomerId,
    #[serde(default)]
    pub expect: Vec<u16>,
    #[serde(default)]
    pub headers: Headers,
    /// Set for an application's probe (see `ServerConfig::probed`), never in `[[endpoints]]`.
    #[serde(skip)]
    pub app: Option<AppKey>,
}

/// Request headers of a probe. They may hold a token, so `Debug` shows only their names.
#[derive(Deserialize, Clone, Default)]
#[serde(transparent)]
pub struct Headers(pub BTreeMap<String, String>);

impl std::ops::Deref for Headers {
    type Target = BTreeMap<String, String>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl fmt::Debug for Headers {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.0.keys()).finish()
    }
}

/// An application the operator cares about: described, probed, and missed when it is gone.
#[derive(Deserialize, Debug, Clone)]
pub struct AppConfig {
    pub id: AppKey,
    pub name: Option<String>,
    pub env: Option<String>,
    pub note: Option<String>,
    #[serde(default)]
    pub probes: Vec<AppProbe>,
}

/// One of an application's URLs; its customer is its host's.
#[derive(Deserialize, Debug, Clone)]
pub struct AppProbe {
    pub url: String,
    #[serde(default)]
    pub expect: Vec<u16>,
    #[serde(default)]
    pub headers: Headers,
}

impl ServerConfig {
    /// Every URL to probe: `[[endpoints]]`, then each application's probes.
    pub fn probed(&self) -> Vec<Endpoint> {
        let customer =
            |host: &str| self.hosts.iter().find(|h| h.id == host).map(|h| h.customer.clone());
        let of_apps = self.apps.iter().flat_map(|a| {
            a.probes.iter().map(|p| Endpoint {
                url: p.url.clone(),
                customer: customer(&a.id.host).unwrap_or_default(),
                expect: p.expect.clone(),
                headers: p.headers.clone(),
                app: Some(a.id.clone()),
            })
        });
        self.endpoints.iter().cloned().chain(of_apps).collect()
    }
}

pub fn load(path: &Path) -> anyhow::Result<ServerConfig> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    parse(&text).with_context(|| format!("invalid {}", path.display()))
}

pub fn parse(text: &str) -> anyhow::Result<ServerConfig> {
    // The parser's own message quotes the offending line, which may hold a probe token.
    let cfg: ServerConfig = toml::from_str(text).map_err(|e| {
        let line = e.span().map(|s| text[..s.start].lines().count().max(1));
        anyhow::anyhow!("{} (line {})", e.message(), line.map_or("?".into(), |l| l.to_string()))
    })?;
    validate(&cfg)?;
    Ok(cfg)
}

fn validate(cfg: &ServerConfig) -> anyhow::Result<()> {
    if !cfg.report_interval.is_positive() {
        bail!("report_interval must be positive");
    }
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
    if let Some(m) = cfg.mute.iter().find(|m| m.code == IncidentCode::Unknown) {
        bail!("mute for {} names an unknown incident code", m.subject);
    }
    let mut apps = BTreeSet::new();
    for a in &cfg.apps {
        if !cfg.hosts.iter().any(|h| h.id == a.id.host) || !apps.insert(&a.id) {
            bail!("app {} is on an unknown host or listed twice", a.id);
        }
    }
    if let Some(m) = cfg.mute.iter().find(|m| matches!(m.subject, Subject::Unknown(_))) {
        bail!("mute names an unknown kind of subject: {}", m.subject);
    }
    let mut urls = BTreeSet::new();
    for e in &cfg.probed() {
        let url = validate_endpoint(e, &customers)?;
        if !urls.insert(url) {
            bail!("endpoint {} is listed twice", e.url);
        }
    }
    Ok(())
}

/// The URL as parsed: `https://a.example` and `https://a.example/` are one endpoint.
fn validate_endpoint(e: &Endpoint, customers: &BTreeSet<&str>) -> anyhow::Result<reqwest::Url> {
    let url = reqwest::Url::parse(&e.url).with_context(|| format!("endpoint {}", e.url))?;
    if !matches!(url.scheme(), "http" | "https") || url.host().is_none() {
        bail!("endpoint {} is not an http(s) URL with a host", e.url);
    }
    if !url.username().is_empty() || url.password().is_some() {
        bail!("endpoint {} has credentials in the URL; use headers", e.url);
    }
    if !customers.contains(e.customer.as_str()) {
        bail!("endpoint {} names unknown customer {:?}", e.url, e.customer);
    }
    if let Some(s) = e.expect.iter().find(|s| !(100..=599).contains(*s)) {
        bail!("endpoint {} expects {s}, which is not an HTTP status", e.url);
    }
    for (name, value) in e.headers.iter() {
        HeaderName::from_bytes(name.as_bytes())
            .with_context(|| format!("endpoint {}: header name {name:?}", e.url))?;
        // The value is a secret: never in an error message.
        HeaderValue::from_str(value).map_err(|_| {
            anyhow::anyhow!("endpoint {}: header {name} has an invalid value", e.url)
        })?;
    }
    Ok(url)
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
            format!("report_interval = \"0s\"\n{}", config("")),
        ];
        for (i, text) in cases.iter().enumerate() {
            assert!(parse(text).is_err(), "case {i} should be rejected");
        }
    }

    fn endpoint(fields: &str) -> String {
        config(&format!("[[endpoints]]\ncustomer = \"acme\"\n{fields}\n"))
    }

    #[test]
    fn endpoints_parse_and_keep_their_headers_out_of_debug() {
        let cfg = parse(&endpoint(
            "url = \"https://shop.example.com/healthz\"\nexpect = [401]\nheaders = { Authorization = \"Bearer s3cret\" }",
        ))
        .unwrap();
        let e = &cfg.endpoints[0];
        assert_eq!(
            (e.expect.as_slice(), e.headers["Authorization"].as_str()),
            ([401].as_slice(), "Bearer s3cret")
        );
        let shown = format!("{cfg:?}");
        assert!(shown.contains("Authorization") && !shown.contains("s3cret"), "{shown}");
        assert!(parse(&endpoint("url = \"http://10.0.0.5:8080/\"")).is_ok());
    }

    #[test]
    fn invalid_endpoints_are_rejected() {
        let cases = [
            endpoint("url = \"not a url\""),
            endpoint("url = \"ftp://shop.example.com/\""),
            endpoint("url = \"https://user:pw@shop.example.com/\""),
            endpoint("url = \"https://user@shop.example.com/\""),
            endpoint("url = \"https://shop.example.com/\"\nexpect = [99]"),
            endpoint("url = \"https://shop.example.com/\"\nexpect = [600]"),
            endpoint("url = \"https://shop.example.com/\"\nheaders = { \"Bad Name\" = \"x\" }"),
            endpoint("url = \"https://shop.example.com/\"\nheaders = { X-Token = \"a\\nb\" }"),
            config("[[endpoints]]\nurl = \"https://a.example/\"\ncustomer = \"nobody\"\n"),
            endpoint(
                "url = \"https://a.example\"\n[[endpoints]]\nurl = \"https://a.example/\"\ncustomer = \"acme\"",
            ),
        ];
        for text in &cases {
            assert!(parse(text).is_err(), "should be rejected:\n{text}");
        }
        let broken = endpoint("url = \"https://a.example/\"\nheaders = { X-Token = \"s3cret\" ");
        let err = parse(&broken).unwrap_err();
        assert!(!format!("{err:#}").contains("s3cret"), "a syntax error does not quote the line");
        assert!(format!("{err:#}").contains("(line "), "{err:#}");
        let err =
            parse(&endpoint("url = \"https://a.example/\"\nheaders = { X-Token = \"s3cret\\n\" }"))
                .unwrap_err();
        assert!(!format!("{err:#}").contains("s3cret"), "a bad value is never echoed");
    }

    fn with_app(app: &str) -> String {
        config(&(host("x", "acme", &hash('a')) + app))
    }

    #[test]
    fn apps_parse_and_their_probes_join_the_endpoints() {
        let cfg = parse(&with_app(
            "[[apps]]\nid = \"x/shop\"\nname = \"Shop\"\nenv = \"prod\"\n\
             [[apps.probes]]\nurl = \"https://shop.example.com/healthz\"\n\
             headers = { Authorization = \"Bearer s3cret\" }\n\
             [[apps]]\nid = \"x/-/redis\"\n\
             [[endpoints]]\nurl = \"https://partner.example.com/\"\ncustomer = \"acme\"\n",
        ))
        .unwrap();
        let probed = cfg.probed();
        let summary: Vec<(&str, &str, Option<String>)> = probed
            .iter()
            .map(|e| (e.url.as_str(), e.customer.as_str(), e.app.as_ref().map(ToString::to_string)))
            .collect();
        assert_eq!(
            summary,
            [
                ("https://partner.example.com/", "acme", None),
                ("https://shop.example.com/healthz", "acme", Some("x/shop".into())),
            ],
            "an app's probe takes its host's customer"
        );
        assert_eq!(probed[1].headers["Authorization"], "Bearer s3cret");
        let shown = format!("{cfg:?}");
        assert!(shown.contains("Authorization") && !shown.contains("s3cret"), "{shown}");
    }

    #[test]
    fn invalid_apps_are_rejected() {
        let cases = [
            with_app("[[apps]]\nid = \"x\"\n"),
            with_app("[[apps]]\nid = \"x/-\"\n"),
            with_app("[[apps]]\nid = \"y/shop\"\n"),
            with_app("[[apps]]\nid = \"x/shop\"\n[[apps]]\nid = \"x/shop\"\n"),
            with_app(
                "[[apps]]\nid = \"x/shop\"\n[[apps.probes]]\nurl = \"https://a.example/\"\n\
                 [[endpoints]]\nurl = \"https://a.example\"\ncustomer = \"acme\"\n",
            ),
            with_app("[[apps]]\nid = \"x/shop\"\n[[apps.probes]]\nurl = \"ftp://a.example/\"\n"),
            with_app("[[mute]]\nsubject = \"queue:mail\"\ncode = \"WORKLOAD_DOWN\"\n"),
        ];
        for text in &cases {
            assert!(parse(text).is_err(), "should be rejected:\n{text}");
        }
        assert!(
            parse(&with_app("[[mute]]\nsubject = \"app:x/shop\"\ncode = \"APP_MISSING\"\n"))
                .is_ok()
        );
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
