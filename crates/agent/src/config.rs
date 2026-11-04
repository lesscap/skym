//! `/etc/skym/config.toml`. Keys this version does not know are ignored.

use anyhow::Context;
use jiff::SignedDuration;
use serde::Deserialize;
use skym_core::model::DatastoreKind;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub const DEFAULT_PATH: &str = "/etc/skym/config.toml";

#[derive(Deserialize, Debug)]
#[serde(default)]
pub struct Config {
    /// Base URL of `skym-server`; only `skym agent` and `skym doctor` need it.
    pub server: Option<String>,
    /// The host token, kept out of this file (mode 0600).
    pub token_file: PathBuf,
    /// Undelivered reports and log cursors.
    pub state_dir: PathBuf,
    pub interval: SignedDuration,
    pub docker: DockerConfig,
    pub systemd: Vec<SystemdUnit>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            server: None,
            token_file: "/etc/skym/token".into(),
            state_dir: "/var/lib/skym".into(),
            interval: SignedDuration::from_secs(60),
            docker: DockerConfig::default(),
            systemd: Vec::new(),
        }
    }
}

#[derive(Deserialize, Debug)]
#[serde(default)]
pub struct DockerConfig {
    pub enabled: bool,
    pub socket: PathBuf,
    pub exclude: Vec<String>,
    pub blue_green: Vec<BlueGreen>,
    pub workloads: Vec<WorkloadConfig>,
}

impl Default for DockerConfig {
    fn default() -> Self {
        DockerConfig {
            enabled: true,
            socket: "/var/run/docker.sock".into(),
            exclude: Vec::new(),
            blue_green: Vec::new(),
            workloads: Vec::new(),
        }
    }
}

impl DockerConfig {
    pub fn workload(&self, project: &str, service: &str) -> Option<&WorkloadConfig> {
        self.workloads.iter().find(|w| w.project == project && w.service == service)
    }
}

/// Two compose projects taking turns to run `services`: one of them may be stopped while
/// the other runs it.
#[derive(Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct BlueGreen {
    pub projects: Vec<String>,
    pub services: Vec<String>,
}

/// What skym cannot tell of one workload by itself.
#[derive(Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct WorkloadConfig {
    pub project: String,
    pub service: String,
    /// The engine of a datastore whose image skym does not recognise.
    #[serde(default)]
    pub datastore: Option<DatastoreKind>,
    /// Caddy request errors for these hosts are not exceptions (retired sites).
    #[serde(default)]
    pub ignore_caddy_hosts: Vec<String>,
}

#[derive(Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct SystemdUnit {
    pub unit: String,
    #[serde(default)]
    pub ports: Vec<u16>,
}

/// An explicit path must exist; the default path may be absent (defaults apply).
pub fn load(path: Option<&Path>) -> anyhow::Result<Config> {
    read(path.unwrap_or(Path::new(DEFAULT_PATH)), path.is_some())
}

fn read(file: &Path, required: bool) -> anyhow::Result<Config> {
    match std::fs::read_to_string(file) {
        Ok(text) => parse(&text).with_context(|| format!("invalid {}", file.display())),
        Err(e) if !required && e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
        Err(e) => Err(e).with_context(|| format!("cannot read {}", file.display())),
    }
}

fn parse(text: &str) -> anyhow::Result<Config> {
    let cfg: Config = toml::from_str(text)?;
    let mut seen = std::collections::BTreeSet::new();
    if let Some(dup) = cfg.systemd.iter().find(|u| !seen.insert(&u.unit)) {
        anyhow::bail!("[[systemd]] unit {:?} is declared twice", dup.unit);
    }
    anyhow::ensure!(cfg.interval.is_positive(), "interval must be positive");
    validate_docker(&cfg.docker)?;
    Ok(cfg)
}

fn validate_docker(d: &DockerConfig) -> anyhow::Result<()> {
    let mut paired = BTreeSet::new();
    for pair in &d.blue_green {
        let [a, b] = pair.projects.as_slice() else {
            anyhow::bail!("[[docker.blue_green]] needs exactly two projects: {:?}", pair.projects);
        };
        anyhow::ensure!(
            !a.is_empty() && !b.is_empty() && a != b,
            "[[docker.blue_green]] projects must be two different names: {:?}",
            pair.projects
        );
        anyhow::ensure!(
            !pair.services.is_empty() && pair.services.iter().all(|s| !s.is_empty()),
            "[[docker.blue_green]] of {:?} needs its services",
            pair.projects
        );
        let services = pair.projects.iter().flat_map(|p| pair.services.iter().map(move |s| (p, s)));
        if let Some((p, s)) = services.into_iter().find(|k| !paired.insert(*k)) {
            anyhow::bail!("{p}/{s} is in more than one [[docker.blue_green]]");
        }
    }
    let mut seen = BTreeSet::new();
    for w in &d.workloads {
        let at = format!("[[docker.workloads]] {}/{}", w.project, w.service);
        anyhow::ensure!(
            !w.project.is_empty() && !w.service.is_empty(),
            "{at}: project and service are required"
        );
        anyhow::ensure!(seen.insert((&w.project, &w.service)), "{at} is declared twice");
        anyhow::ensure!(w.datastore != Some(DatastoreKind::Unknown), "{at}: unknown datastore");
        anyhow::ensure!(w.ignore_caddy_hosts.iter().all(|h| !h.is_empty()), "{at}: empty host");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_documented_example() {
        let cfg = parse(
            r#"
            server = "https://skym.example.com"
            token_file = "/etc/skym/token"
            interval = "30s"

            [docker]
            exclude = ["noisy"]

            [[systemd]]
            unit = "xray"
            ports = [443]

            [report]
            public_ip = false
            "#,
        )
        .unwrap();
        assert_eq!(cfg.server.as_deref(), Some("https://skym.example.com"));
        assert_eq!(cfg.state_dir, Path::new("/var/lib/skym"));
        assert_eq!(cfg.interval, SignedDuration::from_secs(30));
        assert!(cfg.docker.enabled);
        assert_eq!(cfg.docker.exclude, ["noisy"]);
        assert_eq!(cfg.systemd, [SystemdUnit { unit: "xray".into(), ports: vec![443] }]);
    }

    #[test]
    fn missing_default_file_means_defaults_but_explicit_path_must_exist() {
        let missing = Path::new("/nonexistent/skym.toml");
        assert!(load(Some(missing)).is_err());
        assert_eq!(read(missing, false).unwrap().interval, SignedDuration::from_secs(60));
        assert!(read(Path::new("/"), false).is_err(), "unreadable is not missing");
        assert_eq!(parse("").unwrap().interval, SignedDuration::from_secs(60));
        let twice = "[[systemd]]\nunit = \"xray\"\n[[systemd]]\nunit = \"xray\"\n";
        assert!(parse(twice).unwrap_err().to_string().contains("declared twice"));
        assert!(parse("interval = \"0s\"").is_err());
    }

    #[test]
    fn blue_green_pairs_and_workload_overrides() {
        let cfg = parse(
            r#"
            [[docker.blue_green]]
            projects = ["shop", "shop-green"]
            services = ["web", "edge"]

            [[docker.workloads]]
            project = "infra"
            service = "database"
            datastore = "postgres"

            [[docker.workloads]]
            project = "i"
            service = "caddy"
            ignore_caddy_hosts = ["old.example.com"]
            "#,
        )
        .unwrap();
        assert_eq!(cfg.docker.blue_green[0].projects, ["shop", "shop-green"]);
        let db = cfg.docker.workload("infra", "database").unwrap();
        assert_eq!(db.datastore, Some(DatastoreKind::Postgres));
        assert_eq!(
            cfg.docker.workload("i", "caddy").unwrap().ignore_caddy_hosts,
            ["old.example.com"]
        );
        assert!(cfg.docker.workload("i", "database").is_none());
        let rejected = [
            "[[docker.blue_green]]\nprojects = [\"a\"]\nservices = [\"web\"]",
            "[[docker.blue_green]]\nprojects = [\"a\", \"a\"]\nservices = [\"web\"]",
            "[[docker.blue_green]]\nprojects = [\"a\", \"b\"]\nservices = []",
            "[[docker.blue_green]]\nprojects = [\"a\", \"b\"]\nservices = [\"web\"]\n\
             [[docker.blue_green]]\nprojects = [\"a\", \"c\"]\nservices = [\"web\"]",
            "[[docker.workloads]]\nproject = \"a\"\nservice = \"db\"\n\
             [[docker.workloads]]\nproject = \"a\"\nservice = \"db\"",
            "[[docker.workloads]]\nproject = \"a\"\nservice = \"db\"\ndatastore = \"oracle\"",
            "[[docker.workloads]]\nproject = \"\"\nservice = \"db\"",
        ];
        for text in rejected {
            assert!(parse(text).is_err(), "accepted: {text}");
        }
    }
}
