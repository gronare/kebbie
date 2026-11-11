use std::collections::HashSet;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use ed25519_dalek::{Signature, VerifyingKey};
use serde_json::{Value, json};

use super::{BugReport, Bytes32, REPORT_SIGN_DOMAIN, ReportChallenge, ReportUpload, SIGNAL_SIGN_DOMAIN, SignalUpload, meets, signed_bytes};

pub const TOKEN: &str = "kha_fake_inbox";

#[derive(Clone, Debug)]
pub struct StoredReport {
    pub id: u64,
    pub key: [u8; 32],
    pub report: BugReport,
    pub acked: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SignalRow {
    pub version: String,
    pub os: String,
    pub arch: String,
    pub component: String,
    pub kind: String,
    pub count: u32,
    pub vetted: bool,
}

#[derive(Default)]
struct Store {
    difficulty: u8,
    issued: HashSet<[u8; 32]>,
    next_nonce: u64,
    reports: Vec<StoredReport>,
    signals: Vec<SignalRow>,
    allowed: HashSet<[u8; 32]>,
}

pub struct FakeIssues {
    pub url: String,
    store: Arc<Mutex<Store>>,
}

impl FakeIssues {
    pub fn start(difficulty: u8) -> FakeIssues {
        let store = Arc::new(Mutex::new(Store { difficulty, ..Store::default() }));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let serving = Arc::clone(&store);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(stream);
                let mut first = String::new();
                let _ = reader.read_line(&mut first);
                let (mut length, mut bearer) = (0usize, None);
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    let (name, value) = line.split_once(':').unwrap_or((&line, ""));
                    match name.trim().to_ascii_lowercase().as_str() {
                        "content-length" => length = value.trim().parse().unwrap_or(0),
                        "authorization" => bearer = value.trim().strip_prefix("Bearer ").map(str::to_string),
                        _ => {}
                    }
                }
                let mut body = vec![0; length];
                let _ = reader.read_exact(&mut body);
                let mut words = first.split_whitespace();
                let (method, path) = (words.next().unwrap_or("GET"), words.next().unwrap_or("/"));
                let (status, answer) = respond(&mut serving.lock().unwrap(), method, path, bearer.as_deref(), &String::from_utf8_lossy(&body));
                let text = answer.map(|value| value.to_string()).unwrap_or_default();
                let _ = write!(reader.into_inner(), "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}", text.len());
            }
        });
        FakeIssues { url, store }
    }

    pub fn allow(&self, key: &[u8; 32]) {
        self.store.lock().unwrap().allowed.insert(*key);
    }

    pub fn quarantined(&self) -> Vec<StoredReport> {
        let store = self.store.lock().unwrap();
        store.reports.iter().filter(|stored| !store.allowed.contains(&stored.key)).cloned().collect()
    }

    pub fn unacked_allowed(&self) -> Vec<StoredReport> {
        let store = self.store.lock().unwrap();
        store.reports.iter().filter(|stored| store.allowed.contains(&stored.key) && !stored.acked).cloned().collect()
    }

    pub fn lose_acks(&self) {
        self.store.lock().unwrap().reports.iter_mut().for_each(|stored| stored.acked = false);
    }

    pub fn signal_rows(&self) -> Vec<SignalRow> {
        self.store.lock().unwrap().signals.clone()
    }
}

fn refuse(status: &'static str, code: &str) -> (&'static str, Option<Value>) {
    (status, Some(json!({ "error": code })))
}

fn signed(key: &Bytes32, domain: &[u8], nonce: &[u8; 32], digest: &[u8; 32], signature: &[u8; 64]) -> bool {
    VerifyingKey::from_bytes(&key.0).is_ok_and(|key| key.verify_strict(&signed_bytes(domain, nonce, digest), &Signature::from_bytes(signature)).is_ok())
}

fn worked(store: &mut Store, challenge: &ReportChallenge, digest: &[u8; 32], counter: u64) -> bool {
    challenge.difficulty == store.difficulty && store.issued.remove(&challenge.nonce.0) && meets(&challenge.nonce.0, digest, counter, challenge.difficulty)
}

fn respond(store: &mut Store, method: &str, path: &str, bearer: Option<&str>, body: &str) -> (&'static str, Option<Value>) {
    let admin = path.starts_with("/v1/admin/");
    if admin && bearer != Some(TOKEN) {
        return refuse("401 Unauthorized", "unauthorized");
    }
    match (method, path.split('?').next().unwrap_or(path)) {
        ("GET", "/v1/reports/challenge") => {
            store.next_nonce += 1;
            let mut nonce = [0u8; 32];
            nonce[..8].copy_from_slice(&store.next_nonce.to_be_bytes());
            store.issued.insert(nonce);
            let challenge = ReportChallenge { nonce: Bytes32(nonce), difficulty: store.difficulty, expires: u64::MAX, tag: Bytes32([0; 32]) };
            ("200 OK", Some(serde_json::to_value(challenge).unwrap()))
        }
        ("POST", "/v1/reports") => {
            let Ok(upload) = serde_json::from_str::<ReportUpload>(body) else { return refuse("400 Bad Request", "malformed") };
            if let Err(code) = upload.report.check() {
                return refuse("400 Bad Request", code);
            }
            let digest = upload.report.digest();
            if !worked(store, &upload.challenge, &digest, upload.counter) {
                return refuse("400 Bad Request", "bad-work");
            }
            if !signed(&upload.key, REPORT_SIGN_DOMAIN, &upload.challenge.nonce.0, &digest, &upload.signature.0) {
                return refuse("400 Bad Request", "bad-signature");
            }
            let id = store.reports.len() as u64 + 1;
            store.reports.push(StoredReport { id, key: upload.key.0, report: upload.report, acked: false });
            ("200 OK", Some(json!({ "report": id })))
        }
        ("POST", "/v1/signals") => {
            let Ok(upload) = serde_json::from_str::<SignalUpload>(body) else { return refuse("400 Bad Request", "malformed") };
            if let Err(code) = upload.signals.check() {
                return refuse("400 Bad Request", code);
            }
            let digest = upload.signals.digest();
            if !worked(store, &upload.challenge, &digest, upload.counter) {
                return refuse("400 Bad Request", "bad-work");
            }
            if !signed(&upload.key, SIGNAL_SIGN_DOMAIN, &upload.challenge.nonce.0, &digest, &upload.signature.0) {
                return refuse("400 Bad Request", "bad-signature");
            }
            let vetted = store.allowed.contains(&upload.key.0);
            let signals = upload.signals;
            for row in &signals.counts {
                store.signals.push(SignalRow { version: signals.version.to_string(), os: signals.os.to_string(), arch: signals.arch.to_string(), component: row.component.to_string(), kind: row.kind.key().to_string(), count: row.count, vetted });
            }
            ("200 OK", Some(json!({ "rows": signals.counts.len() })))
        }
        ("GET", "/v1/admin/reports") => {
            let after: u64 = path.split_once("after=").and_then(|(_, after)| after.split('&').next()?.parse().ok()).unwrap_or(0);
            let allowed = &store.allowed;
            let reports: Vec<Value> = store.reports.iter().filter(|stored| allowed.contains(&stored.key) && !stored.acked && stored.id > after).take(100).map(|stored| json!({ "id": stored.id, "key": Bytes32(stored.key), "received_at": 0, "report": stored.report })).collect();
            let quarantined = store.reports.iter().filter(|stored| !allowed.contains(&stored.key)).count();
            ("200 OK", Some(json!({ "reports": reports, "quarantined": quarantined })))
        }
        ("POST", ack) if ack.starts_with("/v1/admin/reports/") && ack.ends_with("/ack") => {
            let id: Option<u64> = ack.trim_start_matches("/v1/admin/reports/").trim_end_matches("/ack").parse().ok();
            match store.reports.iter_mut().find(|stored| Some(stored.id) == id) {
                Some(stored) => {
                    stored.acked = true;
                    ("204 No Content", None)
                }
                None => refuse("404 Not Found", "not-found"),
            }
        }
        _ => refuse("404 Not Found", "not-found"),
    }
}
