use std::io::{ErrorKind, Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use rustls::{ClientConnection, ServerConnection, StreamOwned};
use serde::{Deserialize, Serialize};

pub const MAX_FRAME: usize = 1024 * 1024;
pub const CHUNK: usize = 256 * 1024;
// What a run's --send and --fetch files carry in all, so a 2 GiB bundle, the files and the output stay within the 4 GiB the hub relays per link.
pub const MAX_FILES: u64 = 1024 * 1024 * 1024;

// How often a side that has nothing to say sends a beat, and how long the other waits without a word before it gives the link up.
pub const BEAT: Duration = Duration::from_secs(10);
pub const SILENT: Duration = Duration::from_secs(45);

// A byte stream that can wait a bounded time for the next bytes: a TCP socket, a TLS stream on one, or a WebSocket through the hub.
pub trait Timed {
    fn wait_at_most(&self, wait: Option<Duration>) -> std::io::Result<()>;
}

impl Timed for TcpStream {
    fn wait_at_most(&self, wait: Option<Duration>) -> std::io::Result<()> {
        self.set_read_timeout(wait)
    }
}

impl<T: Timed + Read + Write> Timed for StreamOwned<ClientConnection, T> {
    fn wait_at_most(&self, wait: Option<Duration>) -> std::io::Result<()> {
        self.sock.wait_at_most(wait)
    }
}

impl<T: Timed + Read + Write> Timed for StreamOwned<ServerConnection, T> {
    fn wait_at_most(&self, wait: Option<Duration>) -> std::io::Result<()> {
        self.sock.wait_at_most(wait)
    }
}

pub trait Pipe: Read + Write + Timed + Send {}

impl<T: Read + Write + Timed + Send> Pipe for T {}

// TCP keepalive after 15 s of quiet, every 5 s, given up after 3 unanswered, and a write unsent or unacknowledged for SILENT fails, so a link to a machine that went away ends in under a minute even mid-send.
pub fn keep_alive(tcp: &TcpStream) {
    let _ = tcp.set_write_timeout(Some(SILENT));
    use std::os::fd::AsRawFd;
    let fd = tcp.as_raw_fd();
    let set = |level: libc::c_int, name: libc::c_int, value: libc::c_int| {
        // SAFETY: fd is the open socket of tcp, and value lives for the call.
        unsafe { libc::setsockopt(fd, level, name, (&raw const value).cast(), std::mem::size_of::<libc::c_int>() as libc::socklen_t) };
    };
    set(libc::SOL_SOCKET, libc::SO_KEEPALIVE, 1);
    #[cfg(target_os = "linux")]
    set(libc::IPPROTO_TCP, libc::TCP_KEEPIDLE, 15);
    #[cfg(target_os = "macos")]
    set(libc::IPPROTO_TCP, libc::TCP_KEEPALIVE, 15);
    set(libc::IPPROTO_TCP, libc::TCP_KEEPINTVL, 5);
    set(libc::IPPROTO_TCP, libc::TCP_KEEPCNT, 3);
    #[cfg(target_os = "linux")]
    set(libc::IPPROTO_TCP, libc::TCP_USER_TIMEOUT, SILENT.as_millis() as libc::c_int);
}

// What the machine asking says it wants.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Ask {
    Ping,
    Run(RunAsk),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunAsk {
    pub repo: String,
    pub task: String,
    pub commit: String,
    pub remote: Option<RemoteAsk>,
    pub command: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub send: Vec<SendAsk>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fetch: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub alongside: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SendAsk {
    pub name: String,
    pub size: u64,
    pub mode: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteAsk {
    pub url: String,
    pub base: String,
}

// What the machine that runs it says back.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "said", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Said {
    Pong,
    Want { haves: Vec<String> },
    Take,
    Started,
    Entry { path: String, dir: bool, size: u64, mode: u32 },
    Exited { code: i32 },
    Failed { reason: String },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Frame {
    Ask(Ask),
    Said(Said),
    Out(Vec<u8>),
    Err(Vec<u8>),
    Data(Vec<u8>),
    End,
    Signal(i32),
    Beat,
}

impl Frame {
    fn encode(&self) -> std::io::Result<Vec<u8>> {
        let (kind, body): (u8, Vec<u8>) = match self {
            Frame::Ask(ask) => (1, json(ask)?),
            Frame::Said(said) => (2, json(said)?),
            Frame::Out(bytes) => (3, bytes.clone()),
            Frame::Err(bytes) => (4, bytes.clone()),
            Frame::Data(bytes) => (5, bytes.clone()),
            Frame::End => (6, Vec::new()),
            Frame::Signal(signal) => (7, signal.to_be_bytes().to_vec()),
            Frame::Beat => (8, Vec::new()),
        };
        if body.len() > MAX_FRAME {
            return Err(std::io::Error::new(ErrorKind::InvalidInput, "a frame past 1 MiB"));
        }
        let mut frame = Vec::with_capacity(5 + body.len());
        frame.push(kind);
        frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
        frame.extend_from_slice(&body);
        Ok(frame)
    }

    // One whole frame off the front of held, or None while it is still coming.
    pub fn decode(held: &mut Vec<u8>) -> std::io::Result<Option<Frame>> {
        let bad = |what: &str| std::io::Error::new(ErrorKind::InvalidData, what.to_string());
        if held.len() < 5 {
            return Ok(None);
        }
        let len = u32::from_be_bytes([held[1], held[2], held[3], held[4]]) as usize;
        if len > MAX_FRAME {
            return Err(bad("the peer sent a frame past 1 MiB"));
        }
        if held.len() < 5 + len {
            return Ok(None);
        }
        let kind = held[0];
        let body: Vec<u8> = held.drain(..5 + len).skip(5).collect();
        let frame = match kind {
            1 => Frame::Ask(serde_json::from_slice(&body).map_err(|error| bad(&error.to_string()))?),
            2 => Frame::Said(serde_json::from_slice(&body).map_err(|error| bad(&error.to_string()))?),
            3 => Frame::Out(body),
            4 => Frame::Err(body),
            5 => Frame::Data(body),
            6 => Frame::End,
            7 => Frame::Signal(i32::from_be_bytes(body.try_into().map_err(|_| bad("a signal is four bytes"))?)),
            8 => Frame::Beat,
            _ => return Err(bad("the peer sent a frame of a kind this kebbie does not know")),
        };
        Ok(Some(frame))
    }
}

fn json<T: Serialize>(value: &T) -> std::io::Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(std::io::Error::other)
}

pub struct Link {
    pipe: Box<dyn Pipe>,
    held: Vec<u8>,
    heard: Instant,
    said: Instant,
    pub beat: Duration,
    pub silent: Duration,
}

impl Link {
    pub fn new(pipe: Box<dyn Pipe>) -> Link {
        Link { pipe, held: Vec::new(), heard: Instant::now(), said: Instant::now(), beat: BEAT, silent: SILENT }
    }

    pub fn send(&mut self, frame: &Frame) -> std::io::Result<()> {
        self.pipe.write_all(&frame.encode()?)?;
        self.pipe.flush()?;
        self.said = Instant::now();
        Ok(())
    }

    pub fn beat_if_due(&mut self) -> std::io::Result<()> {
        if self.said.elapsed() >= self.beat { self.send(&Frame::Beat) } else { Ok(()) }
    }

    // Whether the other side has said nothing, not even a beat, for longer than it may.
    pub fn gone_quiet(&self) -> bool {
        self.heard.elapsed() >= self.silent
    }

    // The next frame, None when wait ran out first or only a beat came; the peer closing is an error.
    pub fn recv(&mut self, wait: Option<Duration>) -> std::io::Result<Option<Frame>> {
        let mut buffer = [0u8; 64 * 1024];
        loop {
            match Frame::decode(&mut self.held)? {
                Some(Frame::Beat) => return Ok(None),
                Some(frame) => return Ok(Some(frame)),
                None => {}
            }
            self.pipe.wait_at_most(wait)?;
            match self.pipe.read(&mut buffer) {
                Ok(0) => return Err(std::io::Error::new(ErrorKind::UnexpectedEof, "the other machine closed the link")),
                Ok(read) => {
                    self.heard = Instant::now();
                    self.held.extend_from_slice(&buffer[..read]);
                }
                Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => return Ok(None),
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
    }

    // The next frame, beating meanwhile; an error once the other side has gone quiet.
    pub fn expect(&mut self) -> std::io::Result<Frame> {
        self.expect_unless(&|| false)
    }

    // As expect, and an error as soon as halted says this side stops.
    pub fn expect_unless(&mut self, halted: &dyn Fn() -> bool) -> std::io::Result<Frame> {
        loop {
            if halted() {
                return Err(std::io::Error::new(ErrorKind::Interrupted, "this machine is stopping"));
            }
            if let Some(frame) = self.recv(Some(Duration::from_millis(500)))? {
                return Ok(frame);
            }
            if self.gone_quiet() {
                return Err(std::io::Error::new(ErrorKind::TimedOut, format!("the other machine sent nothing for {} s", self.silent.as_secs())));
            }
            self.beat_if_due()?;
        }
    }
}

// Work that may take long, as a fetch, a checkout or a bundle, runs beside beats to the other side, so it knows this machine is still there.
pub fn beating<T: Send>(link: &mut Link, work: impl FnOnce() -> T + Send) -> T {
    std::thread::scope(|scope| {
        let running = scope.spawn(work);
        while !running.is_finished() {
            std::thread::sleep(Duration::from_millis(50));
            let _ = link.beat_if_due();
        }
        running.join().expect("the work does not panic")
    })
}
