//! The `/` filter: terms that must all hold. `host:`, `env:` and `tag:` name a field exactly,
//! `!ok` asks for trouble, and anything else is looked for in the row's text. Case is ignored.
//! A term on a field a list does not have is ignored there.

use skym_core::view::{AppSummary, HostOverview, Status, WorkloadSummary};

#[derive(Debug, Default, PartialEq)]
pub struct Filter(Vec<Term>);

#[derive(Debug, PartialEq)]
enum Term {
    Host(String),
    Env(String),
    Tag(String),
    Trouble,
    Text(String),
}

/// What a row offers to match; `None` is a field it does not have.
#[derive(Default)]
struct Fields<'a> {
    host: Option<&'a str>,
    /// `Some(None)`: it may have an environment, and has none.
    env: Option<Option<&'a str>>,
    tags: Option<&'a [String]>,
    status: Option<Status>,
    text: String,
}

impl Filter {
    pub fn parse(s: &str) -> Filter {
        let term = |word: String| {
            let (field, value) = word.split_once(':').unwrap_or_default();
            let make = match field {
                "host" => Term::Host,
                "env" => Term::Env,
                "tag" => Term::Tag,
                _ if word == "!ok" => return Some(Term::Trouble),
                _ => return Some(Term::Text(word)),
            };
            // A field still being typed (`host:`) holds for every row.
            (!value.is_empty()).then(|| make(value.to_string()))
        };
        Filter(s.split_whitespace().map(str::to_lowercase).filter_map(term).collect())
    }

    pub fn app(&self, a: &AppSummary) -> bool {
        self.holds(&Fields {
            host: Some(&a.key.host),
            env: Some(a.env.as_deref()),
            tags: Some(&a.tags),
            status: Some(a.status),
            text: crate::apps::text(a),
        })
    }

    pub fn host(&self, h: &HostOverview) -> bool {
        self.holds(&Fields {
            host: Some(&h.id),
            env: None,
            tags: Some(&h.tags),
            status: Some(h.status),
            text: format!("{} {}", h.id, h.tags.join(" ")),
        })
    }

    /// A problem: where it is, whether it is an application's (not a host's own), that
    /// application as listed (if it is), and its text. Every problem is trouble, and one of a
    /// host's own has no environment to match.
    pub fn problem(
        &self,
        host: &str,
        of_app: bool,
        app: Option<&AppSummary>,
        host_tags: &[String],
        text: String,
    ) -> bool {
        self.holds(&Fields {
            host: Some(host),
            env: of_app.then(|| app.and_then(|a| a.env.as_deref())),
            tags: Some(app.map_or(host_tags, |a| &a.tags)),
            status: None,
            text,
        })
    }

    pub fn workload(&self, w: &WorkloadSummary, text: String) -> bool {
        self.holds(&Fields {
            host: Some(&w.key.host),
            status: Some(w.status),
            text,
            ..Default::default()
        })
    }

    fn holds(&self, row: &Fields) -> bool {
        let same = |a: &str, b: &str| a.to_lowercase() == b;
        self.0.iter().all(|term| match term {
            Term::Host(h) => row.host.is_none_or(|x| same(x, h)),
            Term::Env(e) => row.env.is_none_or(|x| x.is_some_and(|x| same(x, e))),
            Term::Tag(t) => row.tags.is_none_or(|tags| tags.iter().any(|x| same(x, t))),
            Term::Trouble => row.status.is_none_or(|s| s != Status::Ok),
            Term::Text(w) => row.text.to_lowercase().contains(w.as_str()),
        })
    }
}
