//! Where the server is and how to read it: `SKYM_URL` and `SKYM_TOKEN`, from the environment
//! or from `~/.config/skym/env` (the file the skill uses too). `--server` overrides the URL.

use std::collections::BTreeMap;

#[derive(Debug, PartialEq, Eq)]
pub struct Connection {
    pub server: String,
    pub token: String,
}

/// `KEY=VALUE` lines; `#` comments, an `export ` prefix and quotes are allowed.
pub fn parse_env_file(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.strip_prefix("export ").unwrap_or(l).split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().trim_matches(['"', '\'']).to_string()))
        .collect()
}

/// The environment wins over the file; the flag wins over both for the URL.
pub fn resolve(
    flag: Option<String>,
    env: impl Fn(&str) -> Option<String>,
    file: &BTreeMap<String, String>,
) -> Result<Connection, String> {
    let get = |key: &str| env(key).filter(|v| !v.is_empty()).or_else(|| file.get(key).cloned());
    let server = flag
        .or_else(|| get("SKYM_URL"))
        .ok_or("no server: set SKYM_URL (environment or ~/.config/skym/env) or pass --server")?;
    let token = get("SKYM_TOKEN")
        .ok_or("no reader token: set SKYM_TOKEN in the environment or ~/.config/skym/env")?;
    Ok(Connection { server: server.trim_end_matches('/').to_string(), token })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_file_lines() {
        let file = parse_env_file(
            "# skym\n# SKYM_URL=https://old\nSKYM_URL=https://skym.example.com\nexport SKYM_TOKEN=\"abc\"\n\nBROKEN\n",
        );
        assert_eq!(file["SKYM_URL"], "https://skym.example.com");
        assert_eq!(file["SKYM_TOKEN"], "abc");
        assert_eq!(file.len(), 2);
    }

    #[test]
    fn flag_then_environment_then_file() {
        let file = parse_env_file("SKYM_URL=https://file/\nSKYM_TOKEN=from-file\n");
        let none = |_: &str| None;
        let from_env = |k: &str| (k == "SKYM_TOKEN").then(|| "from-env".to_string());
        assert_eq!(
            resolve(None, none, &file),
            Ok(Connection { server: "https://file".into(), token: "from-file".into() })
        );
        assert_eq!(resolve(None, from_env, &file).unwrap().token, "from-env");
        assert_eq!(resolve(Some("http://flag".into()), none, &file).unwrap().server, "http://flag");
        let empty = |_: &str| Some(String::new());
        assert_eq!(resolve(None, empty, &file).unwrap().token, "from-file", "empty is unset");
        assert!(resolve(None, none, &BTreeMap::new()).unwrap_err().contains("no server"));
        let url_only = parse_env_file("SKYM_URL=https://x\n");
        assert!(resolve(None, none, &url_only).unwrap_err().contains("token"));
    }
}
