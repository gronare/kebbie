use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

// The runner's word for a msgpack-RPC call on nvim's socket: [RPC, socket, method, argument?].
pub const RPC: &str = "nvim-rpc";
pub const LOAD: &str = "return KebbieReview.load()";

#[derive(Debug, PartialEq)]
pub enum Next {
    Load,
    Dismiss(&'static str),
    Wait,
}

// nvim_get_mode answers while nvim waits at a prompt, where nvim_eval and --remote-expr hang; only the hit-enter and more prompts are pressed through, anything else the user is answering waits.
pub fn next(mode: &str, blocking: bool) -> Next {
    match (blocking, mode) {
        (false, _) => Next::Load,
        (true, "r") => Next::Dismiss("<CR>"),
        (true, "rm") => Next::Dismiss("G"),
        _ => Next::Wait,
    }
}

fn str_bytes(out: &mut Vec<u8>, text: &str) {
    let len = text.len();
    if len < 32 {
        out.push(0xa0 | len as u8);
    } else if len < 256 {
        out.extend([0xd9, len as u8]);
    } else {
        out.push(0xda);
        out.extend((len as u16).to_be_bytes());
    }
    out.extend(text.as_bytes());
}

pub fn request(method: &str, argument: Option<&str>) -> Vec<u8> {
    let mut out = vec![0x94, 0x00, 0x01];
    str_bytes(&mut out, method);
    match (method, argument) {
        ("nvim_exec_lua", Some(code)) => {
            out.push(0x92);
            str_bytes(&mut out, code);
            out.push(0x90);
        }
        (_, Some(text)) => {
            out.push(0x91);
            str_bytes(&mut out, text);
        }
        (_, None) => out.push(0x90),
    }
    out
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take(&mut self, count: usize) -> Option<&[u8]> {
        let slice = self.bytes.get(self.at..self.at + count)?;
        self.at += count;
        Some(slice)
    }

    fn number(&mut self, count: usize) -> Option<u64> {
        Some(self.take(count)?.iter().fold(0, |value, byte| (value << 8) | u64::from(*byte)))
    }

    fn text(&mut self, count: usize) -> Option<Value> {
        Some(Value::String(String::from_utf8_lossy(self.take(count)?).to_string()))
    }

    fn list(&mut self, count: usize) -> Option<Value> {
        (0..count).map(|_| self.value()).collect::<Option<Vec<_>>>().map(Value::Array)
    }

    fn map(&mut self, count: usize) -> Option<Value> {
        let mut map = serde_json::Map::new();
        for _ in 0..count {
            let key = self.value()?;
            let value = self.value()?;
            map.insert(key.as_str().map_or_else(|| key.to_string(), str::to_string), value);
        }
        Some(Value::Object(map))
    }

    // None while the message is still incomplete; nvim's buffer and window handles, msgpack extensions, read as null.
    fn value(&mut self) -> Option<Value> {
        let byte = *self.take(1)?.first()?;
        let sized = |reader: &mut Self, count| reader.number(count).map(|size| size as usize);
        match byte {
            0x00..=0x7f => Some(json!(byte)),
            0xe0..=0xff => Some(json!(byte as i8)),
            0xc0 => Some(Value::Null),
            0xc2 => Some(json!(false)),
            0xc3 => Some(json!(true)),
            0xa0..=0xbf => self.text(usize::from(byte & 0x1f)),
            0x90..=0x9f => self.list(usize::from(byte & 0x0f)),
            0x80..=0x8f => self.map(usize::from(byte & 0x0f)),
            0xcc..=0xcf => self.number(1 << (byte - 0xcc)).map(|number| json!(number)),
            0xd0..=0xd3 => self.number(1 << (byte - 0xd0)).map(|number| json!(number as i64)),
            0xca => self.take(4).map(|_| Value::Null),
            0xcb => self.take(8).map(|_| Value::Null),
            0xd9..=0xdb => { let count = sized(self, 1 << (byte - 0xd9))?; self.text(count) }
            0xc4..=0xc6 => { let count = sized(self, 1 << (byte - 0xc4))?; self.take(count).map(|_| Value::Null) }
            0xdc | 0xdd => { let count = sized(self, 2 << (byte - 0xdc))?; self.list(count) }
            0xde | 0xdf => { let count = sized(self, 2 << (byte - 0xde))?; self.map(count) }
            0xd4..=0xd8 => self.take(1 + (1 << (byte - 0xd4))).map(|_| Value::Null),
            0xc7..=0xc9 => { let count = sized(self, 1 << (byte - 0xc7))?; self.take(1 + count).map(|_| Value::Null) }
            0xc1 => None,
        }
    }
}

// The first complete message in bytes and how many bytes it took.
pub fn decode(bytes: &[u8]) -> Option<(Value, usize)> {
    let mut reader = Reader { bytes, at: 0 };
    reader.value().map(|value| (value, reader.at))
}

// A response's result as JSON text; a timeout keeps a stuck nvim from stopping the view.
pub fn call(socket: &str, method: &str, argument: Option<&str>) -> std::result::Result<String, String> {
    let mut stream = UnixStream::connect(socket).map_err(|error| format!("{socket}: {error}"))?;
    let limit = Instant::now() + Duration::from_secs(2);
    stream.set_read_timeout(Some(Duration::from_millis(200))).map_err(|error| error.to_string())?;
    stream.write_all(&request(method, argument)).map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        while let Some((message, used)) = decode(&bytes) {
            bytes.drain(..used);
            if message[0] == 1 {
                return match &message[2] {
                    Value::Null => Ok(message[3].to_string()),
                    error => Err(format!("nvim {method}: {error}")),
                };
            }
        }
        if Instant::now() > limit {
            return Err(format!("nvim {method}: no answer"));
        }
        match stream.read(&mut chunk) {
            Ok(0) => return Err(format!("nvim {method}: the socket closed")),
            Ok(read) => bytes.extend(&chunk[..read]),
            Err(error) if matches!(error.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(error) => return Err(error.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_hit_enter_or_more_prompt_is_pressed_through() {
        assert_eq!(next("n", false), Next::Load);
        assert_eq!(next("i", false), Next::Load, "typing in the comment box is no prompt");
        assert_eq!(next("r", true), Next::Dismiss("<CR>"), "lazy.nvim's Press ENTER");
        assert_eq!(next("rm", true), Next::Dismiss("G"), "the more prompt goes to its end, then hit-enter");
        assert_eq!(next("r?", true), Next::Wait, "a confirm question is the user's to answer");
        assert_eq!(next("n", true), Next::Wait, "getchar() waiting for a key");
    }

    #[test]
    fn requests_encode_and_responses_decode_as_msgpack() {
        assert_eq!(request("nvim_get_mode", None), [&[0x94u8, 0x00, 0x01, 0xad][..], b"nvim_get_mode", &[0x90]].concat());
        assert_eq!(request("nvim_input", Some("<CR>")), [&[0x94u8, 0x00, 0x01, 0xaa][..], b"nvim_input", &[0x91, 0xa4], b"<CR>"].concat());
        assert_eq!(request("nvim_exec_lua", Some(LOAD))[17..19], [0x92, 0xa0 | LOAD.len() as u8]);
        let answer = [&[0x94u8, 0x01, 0x01, 0xc0, 0x82, 0xa4][..], b"mode", &[0xa1], b"r", &[0xa8], b"blocking", &[0xc3]].concat();
        assert_eq!(decode(&answer), Some((json!([1, 1, null, { "mode": "r", "blocking": true }]), answer.len())));
        assert_eq!(decode(&answer[..answer.len() - 1]), None, "a message cut short waits for the rest");
        assert_eq!(decode(&[0x92, 0xd4, 0x00, 0x05, 0xcd, 0x01, 0x00]).map(|(value, _)| value), Some(json!([null, 256])), "a window handle is an extension");
    }
}
