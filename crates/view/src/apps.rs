//! The applications page: grouped by environment, host or tag, each group folded to its
//! applications in trouble unless opened. Pure.

use skym_core::view::{AppSummary, Status};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};

/// The usual environments, in this order; any other comes after them, unset last.
const USUAL: [&str; 3] = ["prod", "pre", "test"];
pub const UNCLASSIFIED: &str = "unclassified";
const UNTAGGED: &str = "untagged";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Grouping {
    #[default]
    Env,
    Host,
    Tag,
}

impl Grouping {
    pub fn next(self) -> Grouping {
        match self {
            Grouping::Env => Grouping::Host,
            Grouping::Host => Grouping::Tag,
            Grouping::Tag => Grouping::Env,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Grouping::Env => "environment",
            Grouping::Host => "host",
            Grouping::Tag => "tag",
        }
    }

    /// The groups an application is in: one, or (by tag) one per tag.
    fn of(self, a: &AppSummary) -> Vec<String> {
        match self {
            Grouping::Env => vec![a.env.clone().unwrap_or(UNCLASSIFIED.into())],
            Grouping::Host => vec![a.key.host.clone()],
            Grouping::Tag if a.tags.is_empty() => vec![UNTAGGED.into()],
            Grouping::Tag => a.tags.clone(),
        }
    }
}

/// How applications are ordered: worst first, or by the memory they use.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sort {
    #[default]
    Problems,
    Memory,
}

/// The memory its workloads report, if any does.
pub fn used_memory(a: &AppSummary) -> Option<u64> {
    a.workloads.iter().filter_map(|w| w.memory_used_bytes).reduce(|x, y| x + y)
}

#[derive(Debug, PartialEq)]
pub struct Group<'a> {
    pub name: String,
    /// All of its applications, worst first, then by name.
    pub all: Vec<&'a AppSummary>,
    pub open: bool,
}

impl<'a> Group<'a> {
    /// What it lists: all of them when open, else those in trouble.
    pub fn listed(&self) -> impl Iterator<Item = &'a AppSummary> + '_ {
        self.all.iter().copied().filter(|a| self.open || a.status != Status::Ok)
    }

    pub fn worst(&self) -> Status {
        self.all.iter().map(|a| a.status).max().unwrap_or(Status::Ok)
    }

    /// The memory all of its applications report, folded ones included.
    pub fn memory(&self) -> Option<u64> {
        self.all.iter().copied().filter_map(used_memory).reduce(|x, y| x + y)
    }
}

/// The groups of the applications `keep` keeps, in order: environments the usual ones first
/// and unclassified last; hosts worst first, `external` last; tags by name, untagged last.
/// `open` holds the opened groups, of every grouping.
pub fn groups<'a>(
    apps: &'a [AppSummary],
    by: Grouping,
    sort: Sort,
    open: &BTreeSet<(Grouping, String)>,
    keep: impl Fn(&AppSummary) -> bool,
) -> Vec<Group<'a>> {
    let mut named: BTreeMap<String, Vec<&AppSummary>> = BTreeMap::new();
    for a in apps.iter().filter(|a| keep(a)) {
        for name in by.of(a) {
            named.entry(name).or_default().push(a);
        }
    }
    let mut groups: Vec<Group> = named
        .into_iter()
        .map(|(name, mut all)| {
            all.sort_by(|a, b| {
                let first = match sort {
                    Sort::Problems => b.status.cmp(&a.status),
                    Sort::Memory => used_memory(b).cmp(&used_memory(a)),
                };
                first.then_with(|| a.name.cmp(&b.name))
            });
            let open = open.contains(&(by, name.clone()));
            Group { name, all, open }
        })
        .collect();
    groups.sort_by_cached_key(|g| {
        let rank = match by {
            Grouping::Env if g.name == UNCLASSIFIED => USUAL.len() + 1,
            Grouping::Env => USUAL.iter().position(|u| *u == g.name).unwrap_or(USUAL.len()),
            Grouping::Host => usize::from(g.name == skym_core::subject::EXTERNAL),
            Grouping::Tag => usize::from(g.name == UNTAGGED),
        };
        let worst = (by == Grouping::Host).then(|| Reverse(g.worst()));
        (rank, worst, g.name.clone())
    });
    groups
}

/// What the filter matches: the name, where it runs, its URLs, its note and its tags.
pub fn text(a: &AppSummary) -> String {
    let urls: Vec<&str> = a.endpoints.iter().map(|e| e.url.as_str()).collect();
    let note = a.note.as_deref().unwrap_or("");
    format!("{} {} {} {note} {}", a.name, a.key, urls.join(" "), a.tags.join(" "))
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
            workloads: vec![],
            deploys: vec![],
            exceptions_1h: 0,
            tags: vec![],
        }
    }

    fn shown(groups: &[Group]) -> Vec<(String, Vec<String>)> {
        let names = |g: &Group| g.listed().map(|a| a.name.clone()).collect();
        groups.iter().map(|g| (g.name.clone(), names(g))).collect()
    }

    fn on(host: &str, name: &str, status: Status, tags: &[&str]) -> AppSummary {
        AppSummary {
            key: format!("{host}/{name}").parse().unwrap(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            ..app(name, None, status)
        }
    }

    fn pairs(list: &[(&str, &[&str])]) -> Vec<(String, Vec<String>)> {
        list.iter()
            .map(|(g, names)| (g.to_string(), names.iter().map(|n| n.to_string()).collect()))
            .collect()
    }

    #[test]
    fn environments_in_order_folded_to_their_trouble() {
        let apps = [
            app("zeta", None, Status::Ok),
            app("shop", Some("prod"), Status::Ok),
            app("blog", Some("prod"), Status::Critical),
            app("shop-test", Some("test"), Status::Ok),
            app("shop-pre", Some("pre"), Status::Warn),
            app("adminer", Some("infra"), Status::Ok),
            app("old", Some("demo"), Status::Ok),
            app("vendor", Some("vendor"), Status::Ok),
        ];
        let none = BTreeSet::new();
        assert_eq!(
            shown(&groups(&apps, Grouping::Env, Sort::Problems, &none, |_| true)),
            pairs(&[
                ("prod", &["blog"]),
                ("pre", &["shop-pre"]),
                ("test", &[]),
                ("demo", &[]),
                ("infra", &[]),
                ("vendor", &[]),
                ("unclassified", &[]),
            ]),
            "a folded group lists only its trouble"
        );
        let open =
            BTreeSet::from([(Grouping::Env, "prod".to_string()), (Grouping::Host, "test".into())]);
        let prod = groups(&apps, Grouping::Env, Sort::Problems, &open, |_| true);
        assert_eq!(shown(&prod[..1]), pairs(&[("prod", &["blog", "shop"])]), "worst first");
        assert!(!prod[2].open, "another grouping's open group of the same name stays its own");
        let filtered =
            groups(&apps, Grouping::Env, Sort::Problems, &none, |a| a.name.starts_with("shop"));
        assert_eq!(filtered.len(), 3, "groups with nothing kept are left out");
        assert_eq!(filtered[0].all.len(), 1);
    }

    #[test]
    fn hosts_worst_first_and_tags_by_name_with_the_rest_last() {
        let apps = [
            on("a", "web", Status::Ok, &["acme"]),
            on("b", "api", Status::Warn, &["acme", "beta"]),
            on("external", "partner", Status::Critical, &[]),
            on("c", "db", Status::Critical, &[]),
        ];
        let all = |by| {
            let g = groups(&apps, by, Sort::Problems, &BTreeSet::new(), |_| true);
            g.iter().map(|g| (g.name.clone(), g.worst())).collect::<Vec<_>>()
        };
        let names = |by| all(by).into_iter().map(|(n, _)| n).collect::<Vec<_>>();
        assert_eq!(names(Grouping::Host), ["c", "b", "a", "external"]);
        assert_eq!(all(Grouping::Host)[1], ("b".into(), Status::Warn));
        assert_eq!(names(Grouping::Tag), ["acme", "beta", "untagged"]);
        let by_tag = groups(&apps, Grouping::Tag, Sort::Problems, &BTreeSet::new(), |_| true);
        assert_eq!(by_tag[0].all.len(), 2, "an app with two tags is in both groups");
        assert_eq!(Grouping::Env.next().next().next(), Grouping::Env);
        assert_eq!(
            [Grouping::Env, Grouping::Host, Grouping::Tag].map(Grouping::label),
            ["environment", "host", "tag"]
        );
    }

    /// An application whose workloads report these amounts of memory (`None`: not reported).
    fn using(name: &str, status: Status, memory: &[Option<u64>]) -> AppSummary {
        let workload = |m: &Option<u64>| {
            serde_json::from_value(serde_json::json!({
                "key": { "host": "x", "project": name, "service": "s" },
                "status": "ok", "run": "running", "memory_used_bytes": m
            }))
            .unwrap()
        };
        AppSummary {
            workloads: memory.iter().map(workload).collect(),
            ..app(name, Some("prod"), status)
        }
    }

    #[test]
    fn memory_is_summed_and_orders_groups_when_asked() {
        let apps = [
            using("big", Status::Ok, &[Some(300), None, Some(200)]),
            using("none", Status::Critical, &[None]),
            using("bare", Status::Ok, &[]),
            using("small", Status::Warn, &[Some(100)]),
            using("also", Status::Ok, &[Some(100)]),
        ];
        assert_eq!(
            apps.iter().map(used_memory).collect::<Vec<_>>(),
            [Some(500), None, None, Some(100), Some(100)]
        );
        let open = BTreeSet::from([(Grouping::Env, "prod".to_string())]);
        let order = |sort| {
            let g = groups(&apps, Grouping::Env, sort, &open, |_| true);
            (g[0].all.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), g[0].memory())
        };
        assert_eq!(
            order(Sort::Problems),
            (vec!["none", "small", "also", "bare", "big"], Some(700))
        );
        assert_eq!(
            order(Sort::Memory).0,
            ["big", "also", "small", "bare", "none"],
            "most first, ties by name, none reported last"
        );
        let silent = groups(&apps[1..3], Grouping::Env, Sort::Memory, &open, |_| true);
        assert_eq!(silent[0].memory(), None);
    }

    #[test]
    fn the_filter_reads_names_hosts_urls_notes_and_tags() {
        let mut a = app("shop", Some("prod"), Status::Ok);
        a.note = Some("the web shop".into());
        a.tags = vec!["acme".into(), "eu".into()];
        assert_eq!(text(&a), "shop x/shop  the web shop acme eu");
    }
}
