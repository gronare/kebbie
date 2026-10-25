use std::io::Read;
use std::path::{Path, PathBuf};

use kebbie_hub_api::Bytes32;
use kebbie_hub_protocol::{AccountFingerprints, AccountKey, Id, PublicKeys, SecretKeys};
use serde::{Deserialize, Serialize};

use crate::sh::Result;

pub const DIR: &str = "hub";
pub const DEV_DIR: &str = "hub-dev";
const FILE: &str = "identity.json";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Account {
    pub key: AccountKey,
    pub fingerprints: AccountFingerprints,
    pub pinned_at: u64,
}

// The machine's keys and token never leave this file, so it has no Debug and nothing prints it.
#[derive(Clone, Serialize, Deserialize)]
pub struct Identity {
    pub url: String,
    pub machine: Id,
    pub name: String,
    token: String,
    signing: Bytes32,
    encryption: Bytes32,
    pub joined_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<Account>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rename_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rename_stopped: Option<String>,
}

impl Account {
    pub fn prints(&self) -> (String, String) {
        (format!("{:?}", Bytes32(self.fingerprints.signing)), format!("{:?}", Bytes32(self.fingerprints.encryption)))
    }
}

pub fn dir(state: &Path) -> PathBuf {
    dir_of(state, super::dev().is_some())
}

pub fn dir_of(state: &Path, dev: bool) -> PathBuf {
    state.join(if dev { DEV_DIR } else { DIR })
}

pub fn random32() -> Result<[u8; 32]> {
    let mut bytes = [0u8; 32];
    std::fs::File::open("/dev/urandom").and_then(|mut source| source.read_exact(&mut bytes)).map_err(|error| format!("/dev/urandom: {error}"))?;
    Ok(bytes)
}

impl Identity {
    pub fn new(url: &str, machine: Id, name: &str, token: String, keys: &SecretKeys, now: u64) -> Identity {
        let (signing, encryption) = keys.to_bytes();
        Identity { url: url.to_string(), machine, name: name.to_string(), token, signing: Bytes32(signing), encryption: Bytes32(encryption), joined_at: now, account: None, rename_error: None, rename_stopped: None }
    }

    pub fn secret(&self) -> SecretKeys {
        SecretKeys::from_bytes(self.signing.0, self.encryption.0)
    }

    pub fn public(&self) -> PublicKeys {
        self.secret().public()
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    pub fn pin(&mut self, key: AccountKey, now: u64) {
        self.account = Some(Account { fingerprints: key.fingerprints(), key, pinned_at: now });
    }
}

pub fn load(state: &Path) -> Result<Option<Identity>> {
    load_at(&dir(state))
}

pub fn load_at(dir: &Path) -> Result<Option<Identity>> {
    let path = dir.join(FILE);
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).map(Some).map_err(|error| format!("{}: {error}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

pub fn save(state: &Path, identity: &Identity) -> Result<()> {
    save_at(&dir(state), identity)
}

pub fn save_at(dir: &Path, identity: &Identity) -> Result<()> {
    let text = serde_json::to_string_pretty(identity).map_err(|error| error.to_string())?;
    crate::state::write(&dir.join(FILE), text)
}

pub fn forget(state: &Path) -> Result<()> {
    match std::fs::remove_dir_all(dir(state)) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(format!("{}: {error}", dir(state).display())),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn the_identity_is_kept_private() {
        let state = crate::sh::test_dir("hub-identity");
        let keys = SecretKeys::generate();
        let identity = Identity::new("https://hub.example.com", Id::new(kebbie_hub_protocol::Prefix::Machine).unwrap(), "mac", "khm_secret".to_string(), &keys, 1);
        save(&state, &identity).unwrap();
        assert_eq!(std::fs::metadata(dir(&state).join(FILE)).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(dir(&state)).unwrap().permissions().mode() & 0o777, 0o700);
        let loaded = load(&state).unwrap().unwrap();
        assert_eq!(loaded.public(), keys.public());
        forget(&state).unwrap();
        assert!(load(&state).unwrap().is_none());
    }
}
