use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::config;
use crate::repo::{name_of, pooled_checkouts, worktree_port};
use crate::sh::Result;
use crate::tasks::slots;

pub struct Pooled {
    pub worktree: String,
    pub port: Option<u16>,
    pub url: Result<Option<String>>,
}

#[derive(Clone, Copy, PartialEq, PartialOrd, Eq, Ord)]
enum Skip {
    Config,
    NoPort,
    NoUrl,
    Local,
    NoHost,
    Taken,
}

impl Skip {
    fn reason(self) -> &'static str {
        match self {
            Skip::Config => "repo config does not load (kebbie doctor)",
            Skip::NoPort => "no port",
            Skip::NoUrl => "no dev_url",
            Skip::Local => "dev_url is on localhost",
            Skip::NoHost => "dev_url names no http(s) host",
            Skip::Taken => "host already routed to another worktree",
        }
    }
}

struct Address<'a> {
    scheme: &'a str,
    host: &'a str,
    port: Option<&'a str>,
}

fn address(url: &str) -> Option<Address<'_>> {
    let (scheme, rest) = url.split_once("://")?;
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit_once('@').map_or(authority, |(_, after)| after);
    let (host, port) = match authority.strip_prefix('[') {
        Some(inner) => {
            let (address, after) = inner.split_once(']')?;
            (&authority[..address.len() + 2], after.strip_prefix(':'))
        }
        None => match authority.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        },
    };
    (!host.is_empty()).then_some(Address { scheme, host, port: port.filter(|port| !port.is_empty()) })
}

fn local(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    host == "localhost" || host.ends_with(".localhost") || host.starts_with("127.") || host == "[::1]" || host == "0.0.0.0"
}

fn site(pooled: &Pooled, plain_http: bool) -> std::result::Result<(String, u16), Skip> {
    let url = pooled.url.as_ref().map_err(|_| Skip::Config)?;
    let port = pooled.port.ok_or(Skip::NoPort)?;
    let url = url.as_deref().ok_or(Skip::NoUrl)?;
    let address = address(url).ok_or(Skip::NoHost)?;
    if local(address.host) {
        return Err(Skip::Local);
    }
    let site = match (plain_http, address.port) {
        (true, _) => format!("http://{}", address.host),
        (false, Some(port)) => format!("{}://{}:{port}", address.scheme, address.host),
        (false, None) => format!("{}://{}", address.scheme, address.host),
    };
    Ok((site, port))
}

pub fn caddyfile(pooled: &[Pooled], left_out: usize, plain_http: bool) -> String {
    let mut taken = BTreeSet::new();
    let mut sites = Vec::new();
    let mut skipped: Vec<(Skip, &str)> = Vec::new();
    for worktree in pooled {
        match site(worktree, plain_http) {
            Ok((site, _)) if taken.contains(&site) => skipped.push((Skip::Taken, &worktree.worktree)),
            Ok((site, port)) => {
                sites.push(format!("{site} {{\n\treverse_proxy localhost:{port}\n}}\n"));
                taken.insert(site);
            }
            Err(skip) => skipped.push((skip, &worktree.worktree)),
        }
    }
    let command = if plain_http { "kebbie routes --plain-http" } else { "kebbie routes" };
    let mut lines = vec![format!("# {command}: a site per pooled worktree with a port in the repos with worktree.dev_route; rerun it when the pool changes\n")];
    lines.extend(sites.iter().cloned());
    let count = match sites.len() { 1 => "1 site".to_string(), n => format!("{n} sites") };
    skipped.sort();
    let mut groups: Vec<String> = Vec::new();
    for (skip, _) in &skipped {
        if groups.last().is_none_or(|last| !last.starts_with(skip.reason())) {
            let names: Vec<&str> = skipped.iter().filter(|(other, _)| other == skip).map(|(_, name)| *name).collect();
            groups.push(format!("{}: {}", skip.reason(), names.join(", ")));
        }
    }
    let mut tail = match skipped.len() {
        0 => format!("# {count}, none skipped"),
        n => format!("# {count}; skipped {n}: {}", groups.join("; ")),
    };
    match left_out {
        0 => {}
        1 => tail.push_str("; 1 repo without worktree.dev_route left out"),
        n => tail.push_str(&format!("; {n} repos without worktree.dev_route left out")),
    }
    lines.push(tail);
    lines.join("\n")
}

pub struct Opted {
    pub dev_route: bool,
    pub dev_url: Option<String>,
}

pub fn gather(repos: &BTreeSet<PathBuf>, opted: impl Fn(&Path) -> Result<Opted>) -> (Vec<Pooled>, usize) {
    let mut found = Vec::new();
    let mut left_out = 0;
    for repo in repos {
        let pool = slots(repo).unwrap_or_default();
        let template = match opted(repo) {
            Ok(Opted { dev_route: false, .. }) => {
                left_out += usize::from(!pool.is_empty());
                continue;
            }
            Ok(Opted { dev_url, .. }) => Ok(dev_url),
            Err(error) => Err(error),
        };
        for slot in pool {
            let url = template.clone().map(|template| config::dev_url(template.as_deref(), repo, &slot.path));
            found.push(Pooled { worktree: name_of(&slot.path), port: worktree_port(&slot.path), url });
        }
    }
    (found, left_out)
}

pub fn print(plain_http: bool) -> String {
    let repos = crate::status::known_repos(&crate::herdr::workspaces().unwrap_or_default(), pooled_checkouts());
    let (pooled, left_out) = gather(&repos, |repo| {
        let settings = config::resolve(Some(repo))?.settings;
        Ok(Opted { dev_route: settings.dev_route.value, dev_url: settings.dev_url.value })
    });
    caddyfile(&pooled, left_out, plain_http)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pooled(worktree: &str, port: Option<u16>, url: Option<&str>) -> Pooled {
        Pooled { worktree: worktree.to_string(), port, url: Ok(url.map(str::to_string)) }
    }

    fn pool() -> Vec<Pooled> {
        vec![
            pooled("wt1.app", Some(3001), Some("https://app-1.dev.example.com")),
            pooled("wt2.app", Some(3002), Some("https://app-2.dev.example.com:8443/admin")),
            pooled("wt3.app", None, Some("https://app-3.dev.example.com")),
            pooled("wt1.api", Some(4001), None),
        ]
    }

    #[test]
    fn a_pool_with_ports_gets_one_site_per_worktree_as_dev_url_names_it() {
        let shown = caddyfile(&pool(), 0, false);
        let expected = "https://app-1.dev.example.com {\n\treverse_proxy localhost:3001\n}\n\nhttps://app-2.dev.example.com:8443 {\n\treverse_proxy localhost:3002\n}\n";
        assert!(shown.contains(expected), "{shown}");
        assert!(shown.starts_with("# kebbie routes:"), "{shown}");
        assert!(shown.ends_with("# 2 sites; skipped 2: no port: wt3.app; no dev_url: wt1.api"), "{shown}");
        assert_eq!(shown, caddyfile(&pool(), 0, false), "the same pool prints the same file");
    }

    #[test]
    fn plain_http_serves_each_host_on_port_80_for_a_proxy_in_front() {
        let shown = caddyfile(&pool(), 0, true);
        assert!(shown.contains("http://app-1.dev.example.com {\n\treverse_proxy localhost:3001\n}\n\nhttp://app-2.dev.example.com {\n\treverse_proxy localhost:3002\n}\n"), "{shown}");
        assert!(!shown.contains("https://") && !shown.contains(":8443"), "{shown}");
        assert!(shown.starts_with("# kebbie routes --plain-http: "), "the file says how to print it again: {shown}");
    }

    #[test]
    fn a_worktree_without_a_routable_host_gets_no_site_and_the_last_line_says_why() {
        let pool = vec![
            pooled("wt1.app", Some(3001), Some("http://localhost:3001")),
            pooled("wt2.app", Some(3002), Some("http://app.dev.example.com")),
            pooled("wt3.app", Some(3003), Some("http://app.dev.example.com")),
            pooled("wt4.app", Some(3004), Some("app-4.dev.example.com")),
            Pooled { worktree: "wt1.api".to_string(), port: Some(4001), url: Err("api.toml: bad".to_string()) },
        ];
        let shown = caddyfile(&pool, 0, false);
        assert_eq!(shown.matches(" {\n").count(), 1, "{shown}");
        assert!(shown.ends_with("# 1 site; skipped 4: repo config does not load (kebbie doctor): wt1.api; dev_url is on localhost: wt1.app; dev_url names no http(s) host: wt4.app; host already routed to another worktree: wt3.app"), "{shown}");
        assert!(caddyfile(&[], 0, true).ends_with("# 0 sites, none skipped"));
        assert!(caddyfile(&[], 2, true).ends_with("# 0 sites, none skipped; 2 repos without worktree.dev_route left out"));
    }

    #[test]
    fn the_host_is_read_from_the_url_without_its_user_path_or_port() {
        let host = |url: &str| address(url).map(|address| (address.host.to_string(), address.port.map(str::to_string)));
        assert_eq!(host("https://u:p@app-1.dev.example.com:8443/x?y#z"), Some(("app-1.dev.example.com".to_string(), Some("8443".to_string()))));
        assert_eq!(host("http://[2001:db8::1]:3000/"), Some(("[2001:db8::1]".to_string(), Some("3000".to_string()))));
        assert_eq!(host("ftp://app.example.com"), None);
        assert_eq!(host("https:///path"), None);
        assert!(local("LOCALHOST") && local("app.localhost") && local("127.0.0.1") && !local("app-1.devbox"));
    }
}
