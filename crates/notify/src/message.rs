//! The text sent to the Feishu group, and the signature of a custom bot. Pure.

use crate::decide::{Kind, Notice};
use base64::Engine;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use skym_core::rules::Severity;

/// Entries shown in full; the rest are counted.
pub const SHOWN: usize = 5;
/// Characters of an incident's detail kept.
pub const DETAIL: usize = 200;

/// One message for a pass. Its first line names skym, for a bot whose security setting is a
/// keyword.
pub fn render(notices: &[Notice]) -> String {
    let mut lines = vec!["skym".to_string()];
    for n in notices.iter().take(SHOWN) {
        let l = &n.label;
        lines.push(format!("{}  {} · {} — {}", head(&n.kind), l.host, l.what, n.code.as_str()));
        if let Some(d) = n.detail.as_deref().filter(|d| !d.is_empty()) {
            lines.push(format!("   {}", cut(d)));
        }
    }
    if notices.len() > SHOWN {
        lines.push(format!("… and {} more", notices.len() - SHOWN));
    }
    lines.join("\n")
}

fn head(kind: &Kind) -> &'static str {
    match kind {
        Kind::New(Severity::Warn | Severity::Info) => "🟡 warn",
        Kind::New(Severity::Critical) => "🔴 critical",
        Kind::New(Severity::Unknown) => "🔴 new",
        Kind::Worse => "🔴 worse",
        Kind::Resolved => "✅ resolved",
    }
}

fn cut(detail: &str) -> String {
    match detail.char_indices().nth(DETAIL) {
        Some((end, _)) => format!("{}…", &detail[..end]),
        None => detail.to_string(),
    }
}

/// `base64(HMAC-SHA256(key = "<timestamp>\n<secret>", message = ""))`, as Feishu checks it.
pub fn sign(secret: &str, timestamp: i64) -> String {
    let key = format!("{timestamp}\n{secret}");
    let mac = Hmac::<Sha256>::new_from_slice(key.as_bytes()).expect("HMAC takes any key");
    base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Label;
    use skym_core::rules::IncidentCode::{DiskFilling, WorkloadDown};

    fn notice(kind: Kind, what: &str, detail: Option<&str>) -> Notice {
        let label = Label { host: "x".into(), what: what.into() };
        Notice { kind, label, code: WorkloadDown, detail: detail.map(String::from) }
    }

    #[test]
    fn a_message_shows_five_then_counts_the_rest() {
        let mut notices = vec![
            notice(Kind::New(Severity::Critical), "hawkeye", Some("hawkeye_task: exited (1)")),
            Notice { code: DiskFilling, ..notice(Kind::New(Severity::Warn), "/data", Some("")) },
            notice(Kind::Worse, "shop", Some("api: down")),
            notice(Kind::Resolved, "caddy", None),
        ];
        assert_eq!(
            render(&notices),
            "skym\n\
             🔴 critical  x · hawkeye — WORKLOAD_DOWN\n   hawkeye_task: exited (1)\n\
             🟡 warn  x · /data — DISK_FILLING\n\
             🔴 worse  x · shop — WORKLOAD_DOWN\n   api: down\n\
             ✅ resolved  x · caddy — WORKLOAD_DOWN"
        );
        notices.push(notice(Kind::Resolved, "app0", None));
        assert!(!render(&notices).contains("more"), "five fit");
        notices.extend((1..3).map(|i| notice(Kind::Resolved, &format!("app{i}"), None)));
        let text = render(&notices);
        assert!(text.contains("app0") && !text.contains("app1"), "{text}");
        assert!(text.ends_with("\n… and 2 more"), "{text}");
    }

    #[test]
    fn a_long_detail_is_cut_at_a_character() {
        let long = "é".repeat(DETAIL + 1);
        let text = render(&[notice(Kind::Worse, "shop", Some(&long))]);
        assert!(text.ends_with(&format!("   {}…", "é".repeat(DETAIL))), "{text}");
        let exact = "é".repeat(DETAIL);
        assert!(render(&[notice(Kind::Worse, "shop", Some(&exact))]).ends_with(&exact));
    }

    #[test]
    fn the_signature_matches_feishus_algorithm() {
        // python3: base64.b64encode(hmac.new(b"1790000000\nsecret", b"", hashlib.sha256).digest())
        assert_eq!(sign("secret", 1_790_000_000), "zJyPHNC4koz2pj1J0av/GAWOBNeVlJOtzZ5lhd1kd+E=");
    }
}
