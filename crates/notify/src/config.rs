//! The notifier's configuration (TOML, kept private): where skym is, and where to tell.

use anyhow::{Context, bail};
use serde::Deserialize;
use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Deserialize, Debug)]
pub struct Config {
    /// skym-server's base URL, e.g. `http://skym:7280`.
    pub server: String,
    /// A reader token of its own.
    pub token: Secret,
    #[serde(default = "default_state")]
    pub state: PathBuf,
    pub feishu: Feishu,
    pub healthchecks: Option<Healthchecks>,
}

#[derive(Deserialize, Debug)]
pub struct Feishu {
    /// The custom bot's webhook; its path holds the bot's token.
    pub webhook: Secret,
    /// The bot's signing secret, when signature verification is on.
    pub secret: Option<Secret>,
}

#[derive(Deserialize, Debug)]
pub struct Healthchecks {
    pub ping_url: Secret,
}

/// A value that must not leave the configuration: `Debug` shows `***`.
#[derive(Deserialize, Clone)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

fn default_state() -> PathBuf {
    "/var/lib/skym-notify/state.json".into()
}

pub fn load(path: &Path) -> anyhow::Result<Config> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    parse(&text).with_context(|| format!("invalid {}", path.display()))
}

/// The parser's own message would quote the offending line, which may hold a secret: only
/// its message and line number are kept.
pub fn parse(text: &str) -> anyhow::Result<Config> {
    let cfg: Config = toml::from_str(text).map_err(|e| {
        let line = e.span().map(|s| text[..s.start].lines().count().max(1));
        anyhow::anyhow!("{} (line {})", e.message(), line.map_or("?".into(), |l| l.to_string()))
    })?;
    validate(&cfg)?;
    Ok(cfg)
}

/// URLs with a host; the secret ones over https. Errors name the key, never the value.
fn validate(cfg: &Config) -> anyhow::Result<()> {
    let url = |key: &str, value: &str, https_only: bool| -> anyhow::Result<()> {
        let parsed = reqwest::Url::parse(value).ok().filter(|u| u.host().is_some());
        let scheme_ok = |s: &str| s == "https" || (!https_only && s == "http");
        if !parsed.is_some_and(|u| scheme_ok(u.scheme())) {
            let kind = if https_only { "an https URL" } else { "an http(s) URL" };
            bail!("{key} is not {kind} with a host");
        }
        Ok(())
    };
    url("server", &cfg.server, false)?;
    url("feishu.webhook", cfg.feishu.webhook.expose(), true)?;
    if let Some(h) = &cfg.healthchecks {
        url("healthchecks.ping_url", h.ping_url.expose(), true)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"
server = "http://skym:7280"
token = "reader-s3cret"
[feishu]
webhook = "https://open.feishu.cn/open-apis/bot/v2/hook/hook-s3cret"
secret = "sign-s3cret"
[healthchecks]
ping_url = "https://hc-ping.com/uuid-s3cret"
"#;

    #[test]
    fn a_valid_configuration_keeps_its_secrets_out_of_debug() {
        let cfg = parse(GOOD).unwrap();
        assert_eq!(cfg.state, PathBuf::from("/var/lib/skym-notify/state.json"));
        assert_eq!(cfg.feishu.secret.as_ref().map(Secret::expose), Some("sign-s3cret"));
        let shown = format!("{cfg:?}");
        assert!(!shown.contains("s3cret") && shown.contains("token: ***"), "{shown}");
        assert!(
            parse(
                &GOOD.replace(
                    "[healthchecks]\nping_url = \"https://hc-ping.com/uuid-s3cret\"\n",
                    ""
                )
            )
            .is_ok()
        );
    }

    #[test]
    fn bad_urls_are_named_never_quoted() {
        let cases = [
            ("http://skym:7280", "ftp://skym", "server"),
            (
                "https://open.feishu.cn/open-apis/bot/v2/hook/hook-s3cret",
                "http://open.feishu.cn/hook-s3cret",
                "feishu.webhook",
            ),
            ("https://hc-ping.com/uuid-s3cret", "not a url uuid-s3cret", "healthchecks.ping_url"),
        ];
        for (good, bad, key) in cases {
            let err = format!("{:#}", parse(&GOOD.replace(good, bad)).unwrap_err());
            assert!(err.contains(key) && !err.contains("s3cret"), "{err}");
        }
        let broken =
            GOOD.replace("token = \"reader-s3cret\"", "token = \"a\"\ntoken = \"again-s3cret\"");
        let err = format!("{:#}", parse(&broken).unwrap_err());
        assert!(err.contains("(line ") && !err.contains("s3cret"), "a syntax error never quotes");
    }
}
