use std::io::{ErrorKind, Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use rustls::{ClientConnection, ServerConnection, StreamOwned};
use serde::{Deserialize, Serialize};

pub const MAX_FRAME: usize = 1024 * 1024;
pub const CHUNK: usize = 256 * 1024;

// A byte stream a link runs over, which can wait a bounded time for the next bytes.
pub trait Pipe: Read + Write + Send {
    fn wait_at_most(&self, wait: Option<Duration>) -> std::io::Result<()>;
}

impl Pipe for StreamOwned<ClientConnection, TcpStream> {
    fn wait_at_most(&self, wait: Option<Duration>) -> std::io::Result<()> {
        self.sock.set_read_timeout(wait)
    }
}

impl Pipe for StreamOwned<ServerConnection, TcpStream> {
    fn wait_at_most(&self, wait: Option<Duration>) -> std::io::Result<()> {
        self.sock.set_read_timeout(wait)
    }
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
    Started,
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
}

impl Link {
    pub fn new(pipe: Box<dyn Pipe>) -> Link {
        Link { pipe, held: Vec::new() }
    }

    pub fn send(&mut self, frame: &Frame) -> std::io::Result<()> {
        self.pipe.write_all(&frame.encode()?)?;
        self.pipe.flush()
    }

    // The next frame, None when wait ran out first; the peer closing is an error.
    pub fn recv(&mut self, wait: Option<Duration>) -> std::io::Result<Option<Frame>> {
        let mut buffer = [0u8; 64 * 1024];
        loop {
            if let Some(frame) = Frame::decode(&mut self.held)? {
                return Ok(Some(frame));
            }
            self.pipe.wait_at_most(wait)?;
            match self.pipe.read(&mut buffer) {
                Ok(0) => return Err(std::io::Error::new(ErrorKind::UnexpectedEof, "the other machine closed the link")),
                Ok(read) => self.held.extend_from_slice(&buffer[..read]),
                Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => return Ok(None),
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
    }

    pub fn expect(&mut self, wait: Duration) -> std::io::Result<Frame> {
        self.recv(Some(wait))?.ok_or_else(|| std::io::Error::new(ErrorKind::TimedOut, format!("the other machine said nothing for {} s", wait.as_secs())))
    }
}
