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

/// The thing a problem is about, for the problems tab: the service, the URL, the mount;
/// nothing when the row already names it (an application, a host).
pub fn what(subject: &Subject) -> String {
    match subject {
        Subject::Workload(k) => k.service.clone(),
        Subject::Endpoint(_) | Subject::Mount { .. } => short(subject),
        Subject::Host(_) | Subject::App(_) | Subject::Unknown(_) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_ways_to_name_a_subject() {
        let named = |s: &str| {
            let s: Subject = s.parse().unwrap();
            (full(&s), short(&s))
        };
        let pair = |a: &str, b: &str| (a.to_string(), b.to_string());
        assert_eq!(named("workload:x/app/api"), pair("app/api", "app/api"));
        assert_eq!(named("workload:x/-/redis"), pair("-/redis", "redis"));
        assert_eq!(named("workload:x/_systemd/xray"), pair("_systemd/xray", "xray"));
        assert_eq!(named("mount:x:/data"), pair("/data", "/data"));
        assert_eq!(named("host:x"), pair("host", "host"));
        let url = "https://shop.example.com/";
        assert_eq!(named(&format!("endpoint:{url}")), pair(url, "shop.example.com"));
        let path = "http://10.0.0.5:8080/healthz";
        assert_eq!(named(&format!("endpoint:{path}")).1, "10.0.0.5:8080/healthz");
        let what = |s: &str| what(&s.parse().unwrap());
        assert_eq!(what("workload:x/app/api"), "api");
        assert_eq!(what(&format!("endpoint:{url}")), "shop.example.com");
        assert_eq!(what("mount:x:/data"), "/data");
        assert_eq!((what("host:x"), what("app:x/shop")), (String::new(), String::new()));
    }
}
