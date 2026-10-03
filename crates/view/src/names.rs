//! How the lists name a subject. Pure.

use skym_core::subject::Subject;

/// `project/service`, a mount's path, `host`: what a filter matches.
pub fn full(subject: &Subject) -> String {
    match subject {
        Subject::Workload(k) => format!("{}/{}", k.project, k.service),
        Subject::Mount { path, .. } => path.clone(),
        Subject::Host(_) => "host".into(),
        Subject::Endpoint(url) => url.clone(),
        Subject::App(a) => a.label().to_string(),
        Subject::Unknown(s) => s.clone(),
    }
}

/// Without a project that says nothing (`-` outside compose, `_systemd`) or a URL's scheme.
pub fn short(subject: &Subject) -> String {
    match subject {
        Subject::Workload(k) if k.project == "-" || k.project == "_systemd" => k.service.clone(),
        Subject::Endpoint(url) => {
            let bare = url.split_once("://").map_or(url.as_str(), |(_, rest)| rest);
            bare.strip_suffix('/').unwrap_or(bare).to_string()
        }
        other => full(other),
    }
}

/// The service alone, where a host column already says where it runs.
pub fn service(subject: &Subject) -> String {
    match subject {
        Subject::Workload(k) => k.service.clone(),
        other => short(other),
    }
}

/// How the overview's left pane names a host or an endpoint.
pub fn target(subject: &Subject) -> String {
    match subject {
        Subject::Host(h) => h.clone(),
        other => short(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_ways_to_name_a_subject() {
        let named = |s: &str| {
            let s: Subject = s.parse().unwrap();
            (full(&s), short(&s), service(&s))
        };
        let triple = |a: &str, b: &str, c: &str| (a.to_string(), b.to_string(), c.to_string());
        assert_eq!(named("workload:x/app/api"), triple("app/api", "app/api", "api"));
        assert_eq!(named("workload:x/-/redis"), triple("-/redis", "redis", "redis"));
        assert_eq!(named("workload:x/_systemd/xray"), triple("_systemd/xray", "xray", "xray"));
        assert_eq!(named("mount:x:/data"), triple("/data", "/data", "/data"));
        assert_eq!(named("host:x"), triple("host", "host", "host"));
        let url = "https://shop.example.com/";
        assert_eq!(
            named(&format!("endpoint:{url}")),
            triple(url, "shop.example.com", "shop.example.com")
        );
        let path = "http://10.0.0.5:8080/healthz";
        assert_eq!(named(&format!("endpoint:{path}")).1, "10.0.0.5:8080/healthz");
        assert_eq!(target(&"host:x".parse().unwrap()), "x");
        assert_eq!(target(&format!("endpoint:{url}").parse().unwrap()), "shop.example.com");
    }
}
