//! The applications page: grouped by environment, the usual ones first. Pure.

use skym_core::view::{AppSummary, Status};
use std::collections::BTreeMap;

/// The usual environments, in this order; any other comes after them, unset last.
const USUAL: [&str; 3] = ["prod", "pre", "test"];

#[derive(Debug, PartialEq)]
pub struct Group<'a> {
    pub env: Option<&'a str>,
    pub apps: Vec<&'a AppSummary>,
    /// Healthy applications of a folded group, not listed.
    pub hidden: usize,
}

/// Groups in environment order. `prod` and unset (to be classified) are open; the others
/// are folded unless `all`, and a folded group still lists its applications in trouble.
pub fn groups<'a>(
    apps: &'a [AppSummary],
    all: bool,
    matches: impl Fn(&AppSummary) -> bool,
) -> Vec<Group<'a>> {
    let mut by_env: BTreeMap<(usize, &str), Vec<&AppSummary>> = BTreeMap::new();
    for a in apps.iter().filter(|a| matches(a)) {
        let env = a.env.as_deref();
        let rank = match env {
            Some(e) => (USUAL.iter().position(|u| *u == e).unwrap_or(USUAL.len()), e),
            None => (USUAL.len() + 1, ""),
        };
        by_env.entry(rank).or_default().push(a);
    }
    by_env
        .into_values()
        .map(|mut list| {
            list.sort_by(|a, b| b.status.cmp(&a.status).then_with(|| a.name.cmp(&b.name)));
            let env = list[0].env.as_deref();
            let folded = !all && env.is_some_and(|e| e != "prod");
            let total = list.len();
            if folded {
                list.retain(|a| a.status != Status::Ok);
            }
            Group { env, hidden: total - list.len(), apps: list }
        })
        .collect()
}

/// What the filter matches: the name, where it runs, its URLs and its note.
pub fn text(a: &AppSummary) -> String {
    let urls: Vec<&str> = a.endpoints.iter().map(|e| e.url.as_str()).collect();
    format!("{} {} {} {}", a.name, a.key, urls.join(" "), a.note.as_deref().unwrap_or(""))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn app(name: &str, env: Option<&str>, status: Status) -> AppSummary {
        AppSummary {
            key: format!("x/{name}").parse().unwrap(),
            name: name.into(),
            env: env.map(String::from),
            note: None,
            configured: env.is_some(),
            status,
            services: 1,
            running: 1,
            last_deployed: None,
            endpoints: vec![],
            incidents: vec![],
            links: BTreeMap::new(),
        }
    }

    fn shown<'a>(groups: &[Group<'a>]) -> Vec<(Option<&'a str>, Vec<&'a str>, usize)> {
        groups
            .iter()
            .map(|g| (g.env, g.apps.iter().map(|a| a.name.as_str()).collect(), g.hidden))
            .collect()
    }

    #[test]
    fn environments_in_order_with_the_rare_ones_folded() {
        let apps = [
            app("zeta", None, Status::Ok),
            app("shop", Some("prod"), Status::Ok),
            app("blog", Some("prod"), Status::Critical),
            app("shop-test", Some("test"), Status::Ok),
            app("shop-pre", Some("pre"), Status::Warn),
            app("adminer", Some("infra"), Status::Ok),
            app("old", Some("demo"), Status::Ok),
        ];
        assert_eq!(
            shown(&groups(&apps, false, |_| true)),
            [
                (Some("prod"), vec!["blog", "shop"], 0),
                (Some("pre"), vec!["shop-pre"], 0),
                (Some("test"), vec![], 1),
                (Some("demo"), vec![], 1),
                (Some("infra"), vec![], 1),
                (None, vec!["zeta"], 0),
            ],
            "worst first; a folded group still lists trouble"
        );
        let all = groups(&apps, true, |_| true);
        assert!(all.iter().all(|g| g.hidden == 0));
        let filtered = groups(&apps, false, |a| a.name.starts_with("shop"));
        assert_eq!(filtered.len(), 3, "groups with nothing to show or hide are left out");
    }

    #[test]
    fn the_filter_reads_names_hosts_urls_and_notes() {
        let mut a = app("shop", Some("prod"), Status::Ok);
        a.note = Some("the web shop".into());
        assert_eq!(text(&a), "shop x/shop  the web shop");
    }
}
