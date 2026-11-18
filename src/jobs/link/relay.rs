use std::io::{ErrorKind, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};
use serde::Deserialize;
use tungstenite::client::IntoClientRequest;
use tungstenite::{Message, WebSocket};

use super::serve::{self, Host};
use super::wire::{Timed, keep_alive};
use crate::hub::identity::Identity;
use crate::sh::Result;

const DIAL: Duration = Duration::from_secs(5);
const ANSWER: Duration = Duration::from_secs(15);
const RETRY_FIRST: Duration = Duration::from_secs(5);
const RETRY_MOST: Duration = Duration::from_secs(60);
const RETRY_OLD_HUB: Duration = Duration::from_secs(600);

// The connection to the hub a relayed link runs on: TLS to an https hub, plain TCP to a local http one.
pub enum HubStream {
    Plain(TcpStream),
    Tls(Box<StreamOwned<ClientConnection, TcpStream>>),
}

impl Read for HubStream {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        match self {
            HubStream::Plain(tcp) => tcp.read(buffer),
            HubStream::Tls(tls) => tls.read(buffer),
        }
    }
}

impl Write for HubStream {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        match self {
            HubStream::Plain(tcp) => tcp.write(bytes),
            HubStream::Tls(tls) => tls.write(bytes),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            HubStream::Plain(tcp) => tcp.flush(),
            HubStream::Tls(tls) => tls.flush(),
        }
    }
}

impl Timed for HubStream {
    fn wait_at_most(&self, wait: Option<Duration>) -> std::io::Result<()> {
        match self {
            HubStream::Plain(tcp) => tcp.set_read_timeout(wait),
            HubStream::Tls(tls) => tls.sock.set_read_timeout(wait),
        }
    }
}

// A WebSocket's binary messages as one byte stream, which the link's own TLS session runs on, so the hub passes only ciphertext.
pub struct WsBytes {
    ws: WebSocket<HubStream>,
    held: Vec<u8>,
    at: usize,
}

fn io_error(error: tungstenite::Error) -> std::io::Error {
    match error {
        tungstenite::Error::Io(error) => error,
        tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed => std::io::Error::new(ErrorKind::UnexpectedEof, "the hub closed the relayed link"),
        other => std::io::Error::other(other.to_string()),
    }
}

impl WsBytes {
    pub fn new(ws: WebSocket<HubStream>) -> WsBytes {
        WsBytes { ws, held: Vec::new(), at: 0 }
    }
}

impl Read for WsBytes {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        while self.at >= self.held.len() {
            match self.ws.read() {
                Ok(Message::Binary(bytes)) => {
                    self.held = bytes.to_vec();
                    self.at = 0;
                }
                Ok(Message::Close(_)) | Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => return Ok(0),
                Ok(_) => {
                    let _ = self.ws.flush();
                }
                Err(error) => return Err(io_error(error)),
            }
        }
        let taken = buffer.len().min(self.held.len() - self.at);
        buffer[..taken].copy_from_slice(&self.held[self.at..self.at + taken]);
        self.at += taken;
        Ok(taken)
    }
}

impl Write for WsBytes {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.ws.write(Message::Binary(bytes.to_vec().into())).map_err(io_error)?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.ws.flush().map_err(io_error)
    }
}

impl Timed for WsBytes {
    fn wait_at_most(&self, wait: Option<Duration>) -> std::io::Result<()> {
        self.ws.get_ref().wait_at_most(wait)
    }
}

fn tls_to_hub() -> Arc<ClientConfig> {
    let roots = RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
    let config = ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider())).with_safe_default_protocol_versions().expect("ring offers the default versions").with_root_certificates(roots).with_no_client_auth();
    Arc::new(config)
}

// Why the hub refused the WebSocket, by its error code: not-listening, no-answer, unknown-machine and the like.
#[derive(Debug, PartialEq)]
pub struct Refused {
    pub status: u16,
    pub code: String,
}

#[derive(Deserialize)]
struct HubError {
    error: String,
}

pub fn open(identity: &Identity, query: &str) -> std::result::Result<WebSocket<HubStream>, std::result::Result<Refused, String>> {
    let base = identity.url.trim_end_matches('/');
    let (secure, rest) = match base.split_once("://") {
        Some(("https", rest)) => (true, rest),
        Some(("http", rest)) => (false, rest),
        _ => return Err(Err(format!("the hub's address {base} is not http or https"))),
    };
    let authority = rest.split('/').next().unwrap_or(rest);
    let host = authority.rsplit_once(':').filter(|(_, port)| port.parse::<u16>().is_ok()).map_or(authority, |(host, _)| host).trim_matches(['[', ']']);
    let port = authority.rsplit_once(':').and_then(|(_, port)| port.parse::<u16>().ok()).unwrap_or(if secure { 443 } else { 80 });
    let addresses: Vec<_> = (host, port).to_socket_addrs().map_err(|error| Err(format!("{host}: {error}")))?.collect();
    let tcp = addresses.iter().find_map(|address| TcpStream::connect_timeout(address, DIAL).ok()).ok_or_else(|| Err(format!("the hub at {base} did not answer")))?;
    keep_alive(&tcp);
    let _ = tcp.set_nodelay(true);
    let _ = tcp.set_read_timeout(Some(ANSWER));
    let stream = if secure {
        let name = ServerName::try_from(host.to_string()).map_err(|error| Err(error.to_string()))?;
        HubStream::Tls(Box::new(StreamOwned::new(ClientConnection::new(tls_to_hub(), name).map_err(|error| Err(error.to_string()))?, tcp)))
    } else {
        HubStream::Plain(tcp)
    };
    let url = format!("{}{rest}/v1/m/link?{query}", if secure { "wss://" } else { "ws://" });
    let mut request = url.into_client_request().map_err(|error| Err(error.to_string()))?;
    let headers = request.headers_mut();
    headers.insert("authorization", format!("Bearer {}", identity.token()).parse().map_err(|_| Err("the machine's token is not a header".to_string()))?);
    headers.insert("user-agent", format!("kebbie/{}", env!("CARGO_PKG_VERSION")).parse().expect("a plain header"));
    match tungstenite::client(request, stream) {
        Ok((ws, _)) => Ok(ws),
        Err(tungstenite::HandshakeError::Failure(tungstenite::Error::Http(response))) => {
            let code = response.body().as_deref().and_then(|body| serde_json::from_slice::<HubError>(body).ok()).map_or_else(|| response.status().to_string(), |said| said.error);
            Err(Ok(Refused { status: response.status().as_u16(), code }))
        }
        Err(error) => Err(Err(format!("the WebSocket to the hub failed: {error}"))),
    }
}

// The hub's host as a person knows it, hub.kebbie.dev from https://hub.kebbie.dev/.
fn hub_host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    rest.split('/').next().unwrap_or(rest)
}

// What an asker hears when the relay gives it no link, in words.
pub fn refusal(name: &str, hub: &str, refused: &std::result::Result<Refused, String>) -> String {
    match refused {
        Ok(Refused { status: 402, code }) if code == crate::hub::http::PRO_REQUIRED => format!("relaying through {} needs Hub Pro; a direct link over Tailscale or WireGuard is free", hub_host(hub)),
        Ok(Refused { code, .. }) if code == "not-listening" => format!("{name} is not waiting for links through the hub either: is its keeper running, with [worker] enabled = true?"),
        Ok(Refused { code, .. }) if code == "no-answer" => format!("{name} was rung through the hub but did not answer within 10 s"),
        Ok(Refused { status: 404 | 405, code }) if code != "unknown-machine" => "the hub relays no links yet; it needs an update".to_string(),
        Ok(Refused { code, .. }) if code == "too-many-links" => "your account has as many links open through the hub as it may".to_string(),
        Ok(Refused { status, code }) => format!("the hub refused the relayed link: {status} {code}"),
        Err(error) => error.clone(),
    }
}

pub fn ask(identity: &Identity, to: &kebbie_hub_protocol::Id, name: &str) -> Result<WsBytes> {
    let ws = open(identity, &format!("to={to}")).map_err(|refused| refusal(name, &identity.url, &refused))?;
    Ok(WsBytes::new(ws))
}

#[derive(Deserialize)]
struct Ring {
    session: String,
    from: String,
}

fn nap(wait: Duration, halted: &dyn Fn() -> bool) {
    let until = Instant::now() + wait;
    while !halted() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(200));
    }
}

// A worker's standing WebSocket to the hub: each ring opens a second one that answers it, and a session runs on that as on a direct link.
pub fn wait_for_rings(host: Arc<Host>, wanted: &dyn Fn() -> bool, halted: Arc<dyn Fn() -> bool + Send + Sync>) {
    let mut retry = RETRY_FIRST;
    let mut said = None::<String>;
    let mut say = |text: String| {
        if said.as_ref() != Some(&text) {
            crate::keeper::log_line(&host.state, &text);
            said = Some(text);
        }
    };
    while !halted() && wanted() {
        match open(&host.identity, "listen=true") {
            Ok(mut ws) => {
                retry = RETRY_FIRST;
                say("link: waiting for links relayed through the hub".to_string());
                let _ = ws.get_ref().wait_at_most(Some(Duration::from_secs(1)));
                let mut checked = Instant::now();
                loop {
                    let stale = checked.elapsed() >= Duration::from_secs(10);
                    if stale {
                        checked = Instant::now();
                    }
                    if halted() || (stale && !wanted()) {
                        let _ = ws.close(None);
                        let _ = ws.flush();
                        return;
                    }
                    match ws.read() {
                        Ok(Message::Text(text)) => {
                            if let Ok(ring) = serde_json::from_str::<Ring>(&text) {
                                let (host, halted) = (host.clone(), halted.clone());
                                std::thread::spawn(move || answer(&host, &ring, &*halted));
                            }
                        }
                        Ok(Message::Close(_)) => break,
                        Ok(_) => {
                            let _ = ws.flush();
                        }
                        Err(tungstenite::Error::Io(error)) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                            let _ = ws.flush();
                        }
                        Err(_) => break,
                    }
                }
                say("link: the hub dropped the wait for relayed links; trying again".to_string());
            }
            Err(Ok(Refused { status: 404 | 405, .. })) => {
                say("link: the hub relays no links yet; it needs an update".to_string());
                retry = RETRY_OLD_HUB;
            }
            Err(refused @ Ok(Refused { status: 402, .. })) => {
                say(format!("link: not waiting for relayed links: {}", refusal("this machine", &host.identity.url, &refused)));
                retry = RETRY_OLD_HUB;
            }
            Err(refused) => {
                say(format!("link: could not wait for relayed links: {}", refusal("this machine", &host.identity.url, &refused)));
                retry = (retry * 2).min(RETRY_MOST);
            }
        }
        nap(retry, &*halted);
    }
}

fn answer(host: &Host, ring: &Ring, halted: &(dyn Fn() -> bool + Sync)) {
    let from = format!("{} through the hub", ring.from);
    match open(&host.identity, &format!("accept={}", ring.session)) {
        Ok(ws) => serve::session(host, from, WsBytes::new(ws), halted),
        Err(refused) => crate::keeper::log_line(&host.state, &format!("link: could not answer a ring from {from}: {}", refusal("this machine", &host.identity.url, &refused))),
    }
}
