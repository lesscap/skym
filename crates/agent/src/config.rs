//! `/etc/skym/config.toml`. Keys this version does not know are ignored.

use anyhow::Context;
use jiff::SignedDuration;
use serde::Deserialize;
use std::path::{Path, PathBuf};

pub const DEFAULT_PATH: &str = "/etc/skym/config.toml";

#[derive(Deserialize, Debug)]
#[serde(default)]
pub struct Config {
    pub interval: SignedDuration,
    pub docker: DockerConfig,
    pub systemd: Vec<SystemdUnit>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
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
}

impl Default for DockerConfig {
    fn default() -> Self {
        DockerConfig { enabled: true, socket: "/var/run/docker.sock".into(), exclude: Vec::new() }
    }
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
    Ok(cfg)
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
    }
}
