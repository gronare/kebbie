use std::io::Read;
use std::path::{Path, PathBuf};

use kelpie_hub_api::Bytes32;
use kelpie_hub_protocol::{Id, PublicKeys, SecretKeys};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::sh::Result;

pub const DIR: &str = "hub";
const FILE: &str = "identity.json";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Device {
    pub id: Id,
    pub keys: PublicKeys,
    pub paired_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
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
    collapse: Bytes32,
    pub joined_at: u64,
    #[serde(default)]
    pub devices: Vec<Device>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rename_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rename_stopped: Option<String>,
}

pub fn dir(state: &Path) -> PathBuf {
    state.join(DIR)
}

fn file(state: &Path) -> PathBuf {
    dir(state).join(FILE)
}

pub fn random32() -> Result<[u8; 32]> {
    let mut bytes = [0u8; 32];
    std::fs::File::open("/dev/urandom").and_then(|mut source| source.read_exact(&mut bytes)).map_err(|error| format!("/dev/urandom: {error}"))?;
    Ok(bytes)
}

impl Identity {
    pub fn new(url: &str, machine: Id, name: &str, token: String, keys: &SecretKeys, now: u64) -> Result<Identity> {
        let (signing, encryption) = keys.to_bytes();
        Ok(Identity { url: url.to_string(), machine, name: name.to_string(), token, signing: Bytes32(signing), encryption: Bytes32(encryption), collapse: Bytes32(random32()?), joined_at: now, devices: Vec::new(), rename_error: None, rename_stopped: None })
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

    // An opaque tag per task, so a newer notice replaces the older one and the push service never learns the task's name.
    pub fn collapse_tag(&self, address: &str) -> String {
        let digest = Sha256::new().chain_update(b"kelpie collapse\0").chain_update(self.collapse.0).chain_update(address.as_bytes()).finalize();
        digest[..16].iter().map(|byte| format!("{byte:02x}")).collect()
    }

    pub fn admit(&mut self, mut device: Device) {
        let label = self.devices.iter().find(|known| known.id == device.id).and_then(|known| known.label.clone());
        device.label = device.label.or(label);
        self.devices.retain(|known| known.id != device.id);
        self.devices.push(device);
    }
}

pub fn load(state: &Path) -> Result<Option<Identity>> {
    let path = file(state);
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).map(Some).map_err(|error| format!("{}: {error}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

pub fn save(state: &Path, identity: &Identity) -> Result<()> {
    let text = serde_json::to_string_pretty(identity).map_err(|error| error.to_string())?;
    crate::state::write(&file(state), text)
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
    fn the_identity_is_kept_private_and_its_collapse_tag_names_no_task() {
        let state = crate::sh::test_dir("hub-identity");
        let keys = SecretKeys::generate();
        let identity = Identity::new("https://hub.example.com", Id::new(kelpie_hub_protocol::Prefix::Machine).unwrap(), "mac", "khm_secret".to_string(), &keys, 1).unwrap();
        save(&state, &identity).unwrap();
        assert_eq!(std::fs::metadata(file(&state)).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(dir(&state)).unwrap().permissions().mode() & 0o777, 0o700);
        let loaded = load(&state).unwrap().unwrap();
        assert_eq!(loaded.public(), keys.public());
        let tag = loaded.collapse_tag("refunds@greenhouse");
        assert_eq!(tag.len(), 32);
        assert_eq!(tag, loaded.collapse_tag("refunds@greenhouse"));
        assert_ne!(tag, loaded.collapse_tag("search@greenhouse"));
        assert!(!tag.contains("refunds"));
        forget(&state).unwrap();
        assert!(load(&state).unwrap().is_none());
    }
}
