use std::cell::RefCell;
use std::fmt;
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;

const MAX_HOPS: usize = 5;
const JSON: &str = "application/json";
pub const CALL: Duration = Duration::from_secs(15);
pub const LONG_POLL: Duration = Duration::from_secs(35);
pub const PRO_REQUIRED: &str = "pro-required";

#[derive(Clone, Debug, PartialEq)]
pub enum HubError {
    Status { status: u16, code: String },
    NotEnabled { url: Option<String> },
    ProRequired { url: Option<String> },
    Transport(String),
    Redirect(String),
}

impl HubError {
    pub fn status(&self) -> Option<u16> {
        match self {
            HubError::Status { status, .. } => Some(*status),
            HubError::NotEnabled { .. } | HubError::ProRequired { .. } => Some(402),
            _ => None,
        }
    }

    // A refusal the same request would get again, so retrying it is pointless.
    pub fn is_final(&self) -> bool {
        self.status().is_some_and(|status| (400..500).contains(&status) && status != 429)
    }
}

impl fmt::Display for HubError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HubError::Status { status, code } => write!(formatter, "the hub answered {status} {code}"),
            HubError::NotEnabled { url: Some(url) } => write!(formatter, "This hub has not enabled answering for your account: {url}"),
            HubError::NotEnabled { url: None } => formatter.write_str("This hub has not enabled answering for your account"),
            HubError::ProRequired { url } => formatter.write_str(&pro_line("That", url.as_deref())),
            HubError::Transport(message) => write!(formatter, "could not reach the hub: {message}"),
            HubError::Redirect(message) => write!(formatter, "the hub's redirect was not followed: {message}"),
        }
    }
}

// What a hub on a plan without Pro says it refused, with where the plan is changed when it names one.
pub fn pro_line(what: &str, url: Option<&str>) -> String {
    url.map_or_else(|| format!("{what} is part of Hub Pro"), |url| format!("{what} is part of Hub Pro: {url}"))
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Method {
    Get,
    Post,
    Put,
    Delete,
}

pub struct Client {
    base: String,
    token: Option<String>,
    agent: ureq::Agent,
    moved: RefCell<Option<String>>,
}

impl Client {
    pub fn new(base: &str, token: Option<&str>, timeout: Duration) -> Client {
        let config = ureq::Agent::config_builder()
            .max_redirects(0)
            .http_status_as_error(false)
            .timeout_global(Some(timeout))
            .timeout_connect(Some(Duration::from_secs(5)))
            .user_agent(format!("kebbie/{}", env!("CARGO_PKG_VERSION")))
            .build();
        Client { base: base.trim_end_matches('/').to_string(), token: token.map(str::to_string), agent: ureq::Agent::new_with_config(config), moved: RefCell::new(None) }
    }

    pub fn moved(&self) -> Option<String> {
        self.moved.borrow().clone()
    }

    pub fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, HubError> {
        parse(&self.send(Method::Get, path, None, JSON)?)
    }

    pub fn post<B: Serialize, T: DeserializeOwned>(&self, path: &str, body: &B) -> Result<T, HubError> {
        parse(&self.send(Method::Post, path, Some(json(body)), JSON)?)
    }

    pub fn put<B: Serialize, T: DeserializeOwned>(&self, path: &str, body: &B) -> Result<T, HubError> {
        parse(&self.send(Method::Put, path, Some(json(body)), JSON)?)
    }

    pub fn put_bytes(&self, path: &str, body: &[u8]) -> Result<(), HubError> {
        parse::<serde::de::IgnoredAny>(&self.send(Method::Put, path, Some(body.to_vec()), "application/octet-stream")?).map(|_| ())
    }

    pub fn delete(&self, path: &str) -> Result<(), HubError> {
        self.send(Method::Delete, path, None, JSON).map(|_| ())
    }

    pub fn reachable(&self) -> Result<(), HubError> {
        self.send(Method::Get, "/healthz", None, JSON).map(|_| ())
    }

    // 307 and 308 keep the method, the body and the token; nothing else is followed.
    fn send(&self, method: Method, path: &str, body: Option<Vec<u8>>, content_type: &str) -> Result<String, HubError> {
        let base = self.moved().unwrap_or_else(|| self.base.clone());
        let mut url = format!("{base}{path}");
        for _ in 0..=MAX_HOPS {
            let mut response = self.once(method, &url, body.as_deref(), content_type)?;
            let status = response.status().as_u16();
            if matches!(status, 307 | 308) {
                let location = response.headers().get("location").and_then(|value| value.to_str().ok()).ok_or_else(|| HubError::Redirect(format!("{status} without a Location")))?;
                let next = follow(&url, location)?;
                if status == 308 && let Some(moved) = next.strip_suffix(path) {
                    *self.moved.borrow_mut() = Some(moved.trim_end_matches('/').to_string());
                }
                url = next;
                continue;
            }
            let text = response.body_mut().read_to_string().map_err(|error| HubError::Transport(error.to_string()))?;
            if (200..300).contains(&status) {
                return Ok(text);
            }
            let body = serde_json::from_str::<serde_json::Value>(&text).unwrap_or_default();
            let code = body["error"].as_str().map(str::to_string).unwrap_or_else(|| response.status().canonical_reason().unwrap_or("error").to_string());
            if status == 402 && code == kebbie_hub_api::PLAN_INACTIVE {
                return Err(HubError::NotEnabled { url: body["url"].as_str().map(str::to_string) });
            }
            if status == 402 && code == PRO_REQUIRED {
                return Err(HubError::ProRequired { url: body["url"].as_str().map(str::to_string) });
            }
            return Err(HubError::Status { status, code });
        }
        Err(HubError::Redirect(format!("more than {MAX_HOPS} redirects")))
    }

    fn once(&self, method: Method, url: &str, body: Option<&[u8]>, content_type: &str) -> Result<ureq::http::Response<ureq::Body>, HubError> {
        let bearer = self.token.as_ref().map(|token| format!("Bearer {token}"));
        let answer = match method {
            Method::Get | Method::Delete => {
                let request = if method == Method::Get { self.agent.get(url) } else { self.agent.delete(url) };
                match &bearer { Some(bearer) => request.header("authorization", bearer).call(), None => request.call() }
            }
            Method::Post | Method::Put => {
                let request = if method == Method::Post { self.agent.post(url) } else { self.agent.put(url) };
                let request = request.content_type(content_type);
                let request = match &bearer { Some(bearer) => request.header("authorization", bearer), None => request };
                request.send(body.unwrap_or_default())
            }
        };
        answer.map_err(|error| HubError::Transport(error.to_string()))
    }
}

fn json<B: Serialize>(body: &B) -> Vec<u8> {
    serde_json::to_vec(body).expect("a request body serializes")
}

fn parse<T: DeserializeOwned>(text: &str) -> Result<T, HubError> {
    let text = if text.trim().is_empty() { "null" } else { text };
    serde_json::from_str(text).map_err(|error| HubError::Transport(format!("the hub's answer did not parse: {error}")))
}

fn origin(url: &str) -> Option<(&str, &str)> {
    let (scheme, rest) = url.split_once("://")?;
    let host = rest.split(['/', '?', '#']).next().filter(|host| !host.is_empty())?;
    Some((scheme, host))
}

fn host_name(host: &str) -> String {
    let name = match host.strip_prefix('[') {
        Some(rest) => rest.split(']').next().unwrap_or_default(),
        None => host.rsplit_once(':').filter(|(_, port)| port.chars().all(|character| character.is_ascii_digit())).map_or(host, |(name, _)| name),
    };
    name.to_ascii_lowercase()
}

// The token and join code go along, so a redirect stays on the hub's own host and never leaves https; another host is the user's call through kebbie hub move.
pub fn follow(from: &str, location: &str) -> Result<String, HubError> {
    let (scheme, host) = origin(from).ok_or_else(|| HubError::Redirect(format!("{from} is not a URL")))?;
    let next = if location.starts_with('/') && !location.starts_with("//") { format!("{scheme}://{host}{location}") } else { location.to_string() };
    let (next_scheme, next_host) = origin(&next).ok_or_else(|| HubError::Redirect(format!("{location} is not a URL")))?;
    if host_name(next_host) != host_name(host) {
        return Err(HubError::Redirect(format!("the hub points to {next_scheme}://{next_host}; run kebbie hub move <url> if you moved it")));
    }
    match (scheme, next_scheme) {
        ("https", "https") | ("http", "http" | "https") => Ok(next),
        _ => Err(HubError::Redirect(format!("{from} redirected to {next}, which leaves https"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_redirect_stays_on_the_hubs_host_and_never_leaves_https() {
        assert_eq!(follow("https://hub.example.com/v1/m/board", "/v2/m/board").unwrap(), "https://hub.example.com/v2/m/board");
        assert_eq!(follow("http://127.0.0.1:8470/v1/m/join", "https://127.0.0.1:8471/v1/m/join").unwrap(), "https://127.0.0.1:8471/v1/m/join");
        assert_eq!(follow("https://Hub.Example.com/v1/m/board", "https://hub.example.com:8443/v1/m/board").unwrap(), "https://hub.example.com:8443/v1/m/board");
        let elsewhere = follow("https://hub.example.com/v1/m/board", "https://hub.example.net/v1/m/board").unwrap_err();
        assert_eq!(elsewhere, HubError::Redirect("the hub points to https://hub.example.net; run kebbie hub move <url> if you moved it".to_string()));
        assert!(matches!(follow("https://hub.example.com/v1/m/board", "https://hub.example.com.evil.example/v1/m/board"), Err(HubError::Redirect(_))));
        assert!(matches!(follow("https://hub.example.com/v1/m/board", "http://hub.example.com/v1/m/board"), Err(HubError::Redirect(_))));
        assert!(matches!(follow("https://hub.example.com/v1/m/board", "//evil.example/v1"), Err(HubError::Redirect(_))));
    }

    #[test]
    fn only_a_refusal_that_would_repeat_is_final() {
        let status = |status| HubError::Status { status, code: String::new() };
        assert!(status(400).is_final() && status(409).is_final() && status(413).is_final());
        assert!(!status(429).is_final() && !status(503).is_final() && !HubError::Transport("down".to_string()).is_final());
        assert!(HubError::ProRequired { url: None }.is_final());
    }

    #[test]
    fn a_refusal_for_want_of_pro_names_hub_pro_never_the_status_code() {
        let refused = HubError::ProRequired { url: Some("https://kebbie.dev/billing".to_string()) };
        assert_eq!(refused.to_string(), "That is part of Hub Pro: https://kebbie.dev/billing");
        assert_eq!(HubError::ProRequired { url: None }.to_string(), "That is part of Hub Pro");
        assert_eq!(pro_line("A second machine", Some("https://kebbie.dev/billing")), "A second machine is part of Hub Pro: https://kebbie.dev/billing");
    }
}
