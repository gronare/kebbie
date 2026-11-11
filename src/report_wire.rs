use std::fmt;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::{Signer, SigningKey};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error};
use sha2::{Digest, Sha256};

// Every domain string, length prefix and field order below is checked byte for byte by the reports endpoint's verifier; tests/fixtures/report-wire pins them.
pub const REPORT_DOMAIN: &[u8] = b"kebbie-report-v1";
pub const REPORT_SIGN_DOMAIN: &[u8] = b"kebbie-report-sign-v1";
pub const REPORT_POW_DOMAIN: &[u8] = b"kebbie-report-pow-v1";
pub const SIGNAL_DOMAIN: &[u8] = b"kebbie-signal-v1";
pub const SIGNAL_SIGN_DOMAIN: &[u8] = b"kebbie-signal-sign-v1";
pub const MAX_TITLE_CHARS: usize = 200;
pub const MAX_TEXT_BYTES: usize = 12 * 1024;
pub const MAX_NAME_CHARS: usize = 48;
pub const MAX_COMMIT_CHARS: usize = 40;
pub const MAX_SIGNAL_ROWS: usize = 64;
pub const MAX_SIGNAL_COUNT: u32 = 10_000;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Bytes32(pub [u8; 32]);

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Bytes64(pub [u8; 64]);

fn decode<'de, D: Deserializer<'de>, const N: usize>(deserializer: D) -> Result<[u8; N], D::Error> {
    let text = String::deserialize(deserializer)?;
    let bytes = URL_SAFE_NO_PAD.decode(&text).map_err(D::Error::custom)?;
    bytes.try_into().map_err(|_| D::Error::custom(format!("expected {N} bytes")))
}

impl fmt::Debug for Bytes32 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&URL_SAFE_NO_PAD.encode(self.0))
    }
}

impl Serialize for Bytes32 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&URL_SAFE_NO_PAD.encode(self.0))
    }
}

impl<'de> Deserialize<'de> for Bytes32 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Bytes32, D::Error> {
        decode(deserializer).map(Bytes32)
    }
}

impl fmt::Debug for Bytes64 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&URL_SAFE_NO_PAD.encode(self.0))
    }
}

impl Serialize for Bytes64 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&URL_SAFE_NO_PAD.encode(self.0))
    }
}

impl<'de> Deserialize<'de> for Bytes64 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Bytes64, D::Error> {
        decode(deserializer).map(Bytes64)
    }
}

fn field(bytes: &mut Vec<u8>, text: &str) {
    bytes.extend((text.len() as u32).to_be_bytes());
    bytes.extend(text.as_bytes());
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BugReport {
    pub title: String,
    pub text: String,
    pub version: String,
    pub os: String,
}

impl BugReport {
    pub fn bytes(&self) -> Vec<u8> {
        let mut bytes = REPORT_DOMAIN.to_vec();
        for text in [&self.title, &self.text, &self.version, &self.os] {
            field(&mut bytes, text);
        }
        bytes
    }

    pub fn digest(&self) -> [u8; 32] {
        Sha256::digest(self.bytes()).into()
    }
}

// The only text a signal carries: lowercase ASCII letters and digits, with . _ + - between them, so no sentence, path or message fits.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Name(String);

impl Name {
    pub fn new(text: &str) -> Option<Name> {
        let first = text.chars().next()?;
        let last = text.chars().next_back()?;
        let inner = |character: char| character.is_ascii_lowercase() || character.is_ascii_digit() || matches!(character, '.' | '_' | '+' | '-');
        let end = |character: char| character.is_ascii_lowercase() || character.is_ascii_digit();
        (text.len() <= MAX_NAME_CHARS && end(first) && end(last) && text.chars().all(inner)).then(|| Name(text.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Name {
    type Error = String;

    fn try_from(text: String) -> Result<Name, String> {
        Name::new(&text).ok_or_else(|| "not a signal name".to_string())
    }
}

impl From<Name> for String {
    fn from(name: Name) -> String {
        name.0
    }
}

impl fmt::Display for Name {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SignalKind {
    Panic,
    InternalError,
    CorruptState,
}

impl SignalKind {
    pub fn key(self) -> &'static str {
        match self {
            SignalKind::Panic => "panic",
            SignalKind::InternalError => "internal-error",
            SignalKind::CorruptState => "corrupt-state",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignalCount {
    pub component: Name,
    pub kind: SignalKind,
    pub count: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorSignals {
    pub version: Name,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<Name>,
    pub os: Name,
    pub arch: Name,
    pub counts: Vec<SignalCount>,
}

impl ErrorSignals {
    pub fn bytes(&self) -> Vec<u8> {
        let mut bytes = SIGNAL_DOMAIN.to_vec();
        field(&mut bytes, self.version.as_str());
        field(&mut bytes, self.commit.as_ref().map_or("", Name::as_str));
        field(&mut bytes, self.os.as_str());
        field(&mut bytes, self.arch.as_str());
        bytes.extend((self.counts.len() as u32).to_be_bytes());
        for row in &self.counts {
            field(&mut bytes, row.component.as_str());
            field(&mut bytes, row.kind.key());
            bytes.extend(row.count.to_be_bytes());
        }
        bytes
    }

    pub fn digest(&self) -> [u8; 32] {
        Sha256::digest(self.bytes()).into()
    }
}

fn pow_hash(nonce: &[u8; 32], digest: &[u8; 32], counter: u64) -> [u8; 32] {
    Sha256::new().chain_update(REPORT_POW_DOMAIN).chain_update(nonce).chain_update(digest).chain_update(counter.to_be_bytes()).finalize().into()
}

fn leading_zero_bits(hash: &[u8; 32]) -> u32 {
    let mut bits = 0;
    for byte in hash {
        if *byte != 0 {
            return bits + byte.leading_zeros();
        }
        bits += 8;
    }
    bits
}

fn meets(nonce: &[u8; 32], digest: &[u8; 32], counter: u64, difficulty: u8) -> bool {
    leading_zero_bits(&pow_hash(nonce, digest, counter)) >= u32::from(difficulty)
}

pub fn solve(nonce: &[u8; 32], digest: &[u8; 32], difficulty: u8) -> u64 {
    (0..u64::MAX).find(|counter| meets(nonce, digest, *counter, difficulty)).expect("some counter meets any difficulty up to 64 bits")
}

fn signed_bytes(domain: &[u8], nonce: &[u8; 32], digest: &[u8; 32]) -> Vec<u8> {
    let mut bytes = domain.to_vec();
    bytes.extend(nonce);
    bytes.extend(digest);
    bytes
}

#[derive(Clone)]
pub struct InstallKey(SigningKey);

impl InstallKey {
    pub fn from_bytes(secret: [u8; 32]) -> InstallKey {
        InstallKey(SigningKey::from_bytes(&secret))
    }

    pub fn to_bytes(&self) -> [u8; 32] {
        self.0.to_bytes()
    }

    pub fn public(&self) -> [u8; 32] {
        self.0.verifying_key().to_bytes()
    }

    pub fn sign(&self, nonce: &[u8; 32], report: &BugReport) -> [u8; 64] {
        self.0.sign(&signed_bytes(REPORT_SIGN_DOMAIN, nonce, &report.digest())).to_bytes()
    }

    pub fn sign_signals(&self, nonce: &[u8; 32], signals: &ErrorSignals) -> [u8; 64] {
        self.0.sign(&signed_bytes(SIGNAL_SIGN_DOMAIN, nonce, &signals.digest())).to_bytes()
    }
}

impl fmt::Debug for InstallKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("InstallKey").field("public", &self.public()).finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportChallenge {
    pub nonce: Bytes32,
    pub difficulty: u8,
    pub expires: u64,
    pub tag: Bytes32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportUpload {
    pub report: BugReport,
    pub key: Bytes32,
    pub signature: Bytes64,
    pub challenge: ReportChallenge,
    pub counter: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub struct ReportAccepted {
    pub report: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignalUpload {
    pub signals: ErrorSignals,
    pub key: Bytes32,
    pub signature: Bytes64,
    pub challenge: ReportChallenge,
    pub counter: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub struct SignalsAccepted {
    pub rows: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct ReceivedReport {
    pub id: u64,
    pub key: Bytes32,
    pub received_at: u64,
    pub report: BugReport,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct ReportInbox {
    pub reports: Vec<ReceivedReport>,
    pub quarantined: u64,
}

#[cfg(test)]
pub mod fake;

#[cfg(test)]
fn short_line(text: &str, limit: usize) -> bool {
    !text.trim().is_empty() && text.chars().count() <= limit && !text.chars().any(char::is_control)
}

// The reports endpoint's own checks, answering with its refusal keys, so a test can prove what kebbie builds passes them.
#[cfg(test)]
impl BugReport {
    pub fn check(&self) -> Result<(), &'static str> {
        const MAX_FIELD_CHARS: usize = 64;
        if self.title.chars().count() > MAX_TITLE_CHARS || self.text.len() > MAX_TEXT_BYTES {
            return Err("too-large");
        }
        if !short_line(&self.title, MAX_TITLE_CHARS) {
            return Err("bad-title");
        }
        if !short_line(&self.version, MAX_FIELD_CHARS) || !short_line(&self.os, MAX_FIELD_CHARS) {
            return Err("bad-field");
        }
        Ok(())
    }
}

#[cfg(test)]
impl ErrorSignals {
    pub fn check(&self) -> Result<(), &'static str> {
        if self.counts.is_empty() {
            return Err("no-signals");
        }
        if self.counts.len() > MAX_SIGNAL_ROWS {
            return Err("too-many-signals");
        }
        if self.counts.iter().any(|row| row.count == 0 || row.count > MAX_SIGNAL_COUNT) {
            return Err("bad-count");
        }
        let mut seen = std::collections::HashSet::new();
        if !self.counts.iter().all(|row| seen.insert((&row.component, row.kind))) {
            return Err("repeated-signal");
        }
        if self.commit.as_ref().is_some_and(|commit| commit.as_str().len() > MAX_COMMIT_CHARS || !commit.as_str().chars().all(|character| character.is_ascii_hexdigit())) {
            return Err("bad-commit");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::{Signature, VerifyingKey};
    use serde_json::Value;

    use super::*;

    const REPORTS: &str = include_str!("../tests/fixtures/report-wire/reports.json");
    const UPLOADS: &str = include_str!("../tests/fixtures/report-wire/uploads.json");

    fn bytes<const N: usize>(value: &Value) -> [u8; N] {
        URL_SAFE_NO_PAD.decode(value.as_str().unwrap()).unwrap().try_into().unwrap()
    }

    fn verifies(public: &[u8; 32], domain: &[u8], nonce: &[u8; 32], digest: &[u8; 32], signature: &[u8; 64]) -> bool {
        VerifyingKey::from_bytes(public).unwrap().verify_strict(&signed_bytes(domain, nonce, digest), &Signature::from_bytes(signature)).is_ok()
    }

    fn report() -> BugReport {
        BugReport { title: "push hid git's error".to_string(), text: "Ran kebbie push; nothing showed.".to_string(), version: "0.26.0 (e58e84a)".to_string(), os: "macos aarch64".to_string() }
    }

    #[test]
    fn the_report_vector_replays_byte_for_byte() {
        let vector: Value = serde_json::from_str(REPORTS).unwrap();
        assert_eq!((vector["report_domain"].as_str().unwrap().as_bytes(), vector["sign_domain"].as_str().unwrap().as_bytes(), vector["pow_domain"].as_str().unwrap().as_bytes()), (REPORT_DOMAIN, REPORT_SIGN_DOMAIN, REPORT_POW_DOMAIN));
        let report: BugReport = serde_json::from_value(vector["report"].clone()).unwrap();
        assert_eq!(URL_SAFE_NO_PAD.decode(vector["report_bytes"].as_str().unwrap()).unwrap(), report.bytes());
        assert_eq!(bytes::<32>(&vector["digest"]), report.digest());
        let key = InstallKey::from_bytes(bytes(&vector["install_secret"]));
        assert_eq!(key.public(), bytes::<32>(&vector["install_public"]));
        let nonce: [u8; 32] = bytes(&vector["nonce"]);
        assert_eq!(key.sign(&nonce, &report), bytes::<64>(&vector["signature"]));
        let difficulty = vector["difficulty"].as_u64().unwrap() as u8;
        let counter = vector["counter"].as_u64().unwrap();
        assert_eq!(solve(&nonce, &report.digest(), difficulty), counter);
        assert_eq!(pow_hash(&nonce, &report.digest(), counter), bytes::<32>(&vector["hash"]));
        assert_eq!(u64::from(leading_zero_bits(&bytes(&vector["hash"]))), vector["hash_zero_bits"].as_u64().unwrap());
        let short = vector["short_counter"].as_u64().unwrap();
        assert_eq!(u64::from(leading_zero_bits(&pow_hash(&nonce, &report.digest(), short))), vector["short_zero_bits"].as_u64().unwrap());
    }

    #[test]
    fn the_report_and_signal_uploads_serialize_to_the_verifiers_fixture_exactly() {
        let fixture: Value = serde_json::from_str(UPLOADS).unwrap();
        let text = |key: &str| fixture[key].as_str().unwrap().to_string();
        let challenge: ReportChallenge = serde_json::from_str(&text("challenge")).unwrap();
        assert_eq!(serde_json::to_string(&challenge).unwrap(), text("challenge"));
        let key = InstallKey::from_bytes([3; 32]);
        let report: ReportUpload = serde_json::from_str(&text("report_upload")).unwrap();
        let built = ReportUpload { signature: Bytes64(key.sign(&challenge.nonce.0, &report.report)), key: Bytes32(key.public()), counter: solve(&challenge.nonce.0, &report.report.digest(), challenge.difficulty), report: report.report.clone(), challenge };
        assert_eq!(serde_json::to_string(&built).unwrap(), text("report_upload"));
        assert_eq!(URL_SAFE_NO_PAD.encode(built.report.digest()), text("report_digest"));
        let signals: SignalUpload = serde_json::from_str(&text("signal_upload")).unwrap();
        let built = SignalUpload { signature: Bytes64(key.sign_signals(&challenge.nonce.0, &signals.signals)), key: Bytes32(key.public()), counter: solve(&challenge.nonce.0, &signals.signals.digest(), challenge.difficulty), signals: signals.signals.clone(), challenge };
        assert_eq!(serde_json::to_string(&built).unwrap(), text("signal_upload"));
        assert_eq!(URL_SAFE_NO_PAD.encode(built.signals.bytes()), text("signal_bytes"));
        assert!(verifies(&built.key.0, SIGNAL_SIGN_DOMAIN, &challenge.nonce.0, &built.signals.digest(), &built.signature.0));
        let inbox: ReportInbox = serde_json::from_str(&text("inbox")).unwrap();
        assert_eq!((inbox.quarantined, inbox.reports[0].id, inbox.reports[0].key, inbox.reports[0].received_at, &inbox.reports[0].report), (2, 1, Bytes32(key.public()), 1_793_000_000, &report.report));
    }

    #[test]
    fn a_solved_counter_meets_its_difficulty_and_the_one_before_does_not() {
        let digest = report().digest();
        let nonce = [7u8; 32];
        let counter = solve(&nonce, &digest, 12);
        assert!(meets(&nonce, &digest, counter, 12));
        assert!((0..counter).all(|earlier| !meets(&nonce, &digest, earlier, 12)));
        let mut hash = [0xffu8; 32];
        assert_eq!(leading_zero_bits(&hash), 0);
        hash[0] = 0;
        hash[1] = 0x1f;
        assert_eq!(leading_zero_bits(&hash), 11);
        assert_eq!(leading_zero_bits(&[0; 32]), 256);
    }

    #[test]
    fn a_report_signature_covers_the_nonce_and_every_field_and_is_no_signal_signature() {
        let key = InstallKey::from_bytes([3u8; 32]);
        let nonce = [9u8; 32];
        let signature = key.sign(&nonce, &report());
        assert!(verifies(&key.public(), REPORT_SIGN_DOMAIN, &nonce, &report().digest(), &signature));
        assert!(!verifies(&key.public(), SIGNAL_SIGN_DOMAIN, &nonce, &report().digest(), &signature));
        assert!(!verifies(&key.public(), REPORT_SIGN_DOMAIN, &[1u8; 32], &report().digest(), &signature));
        for change in [|report: &mut BugReport| report.title.push('!'), |report: &mut BugReport| report.text.push('!'), |report: &mut BugReport| report.version.push('!'), |report: &mut BugReport| report.os.push('!')] {
            let mut changed = report();
            change(&mut changed);
            assert!(!verifies(&key.public(), REPORT_SIGN_DOMAIN, &nonce, &changed.digest(), &signature));
        }
        let moved = BugReport { title: "ab".to_string(), text: "c".to_string(), ..report() };
        assert_ne!(moved.bytes(), BugReport { title: "a".to_string(), text: "bc".to_string(), ..report() }.bytes(), "fields are length-prefixed");
    }

    #[test]
    fn a_signal_payload_takes_no_field_beyond_its_own_and_no_free_text_in_any() {
        let signals = ErrorSignals { version: Name::new("0.26.3").unwrap(), commit: Name::new("16983b9"), os: Name::new("macos").unwrap(), arch: Name::new("aarch64").unwrap(), counts: vec![SignalCount { component: Name::new("keeper.push").unwrap(), kind: SignalKind::InternalError, count: 3 }] };
        let good = serde_json::to_value(&signals).unwrap();
        assert_eq!(serde_json::from_value::<ErrorSignals>(good.clone()).unwrap(), signals);
        let mut changes: Vec<Value> = Vec::new();
        for (path, value) in [(&["message"][..], "push failed in ~/work/app"), (&["counts", "0", "detail"], "x"), (&["counts", "0", "component"], "keeper.push: git refused /Users/ada/app"), (&["counts", "0", "kind"], "something-else"), (&["os"], "macOS 15.1 (ada's laptop)")] {
            let mut changed = good.clone();
            let mut at = &mut changed;
            for step in path {
                at = match step.parse::<usize>() {
                    Ok(index) => &mut at[index],
                    Err(_) => &mut at[*step],
                };
            }
            *at = value.into();
            changes.push(changed);
        }
        assert!(changes.into_iter().all(|changed| serde_json::from_value::<ErrorSignals>(changed).is_err()));
        for text in ["push failed: git said no", "/Users/ada/work", "~/.ssh", "Board.Draw", ".hidden", "trailing-", "a\nb", &"a".repeat(MAX_NAME_CHARS + 1)] {
            assert!(Name::new(text).is_none(), "{text:?}");
        }
    }
}
