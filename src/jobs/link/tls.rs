use std::collections::BTreeMap;
use std::sync::Arc;

use kebbie_hub_protocol::{Id, SecretKeys};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms, verify_tls13_signature_with_raw_key};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, SubjectPublicKeyInfoDer, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::sign::CertifiedKey;
use rustls::{CertificateError, ClientConfig, DigitallySignedStruct, DistinguishedName, Error, ServerConfig, SignatureScheme};

use crate::sh::Result;

// DER for an Ed25519 SubjectPublicKeyInfo and a PKCS#8 private key, each a fixed prefix and the 32 key bytes (RFC 8410).
const SPKI_PREFIX: [u8; 12] = [0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00];
const PKCS8_PREFIX: [u8; 16] = [0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20];

pub fn spki(signing: &[u8; 32]) -> Vec<u8> {
    [SPKI_PREFIX.as_slice(), signing].concat()
}

pub fn key_of(spki: &[u8]) -> Option<[u8; 32]> {
    spki.strip_prefix(SPKI_PREFIX.as_slice()).and_then(|key| key.try_into().ok())
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

// The machine's own Ed25519 signing key as a raw public key (RFC 7250): no certificate, the key on the account's list is the identity.
fn certified(secret: &SecretKeys) -> Result<Arc<CertifiedKey>> {
    let (signing, _) = secret.to_bytes();
    let pkcs8 = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from([PKCS8_PREFIX.as_slice(), &signing].concat()));
    let key = provider().key_provider.load_private_key(pkcs8).map_err(|error| format!("the machine's key did not load: {error}"))?;
    Ok(Arc::new(CertifiedKey::new(vec![CertificateDer::from(spki(&secret.public().signing))], key)))
}

// Which machine an Ed25519 key belongs to, asked only once a peer presents one.
pub type Lookup = Arc<dyn Fn(&[u8; 32]) -> Option<Id> + Send + Sync>;

pub fn fixed(allowed: BTreeMap<[u8; 32], Id>) -> Lookup {
    Arc::new(move |key| allowed.get(key).cloned())
}

// Accepts a peer only by its Ed25519 key, and only a key the lookup knows: the signing keys of a list the account signed and that is no older than one this machine saw.
pub struct Pinned {
    lookup: Lookup,
    algorithms: WebPkiSupportedAlgorithms,
}

impl std::fmt::Debug for Pinned {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Pinned")
    }
}

impl Pinned {
    pub fn new(lookup: Lookup) -> Pinned {
        Pinned { lookup, algorithms: provider().signature_verification_algorithms }
    }

    pub fn check(&self, presented: &[u8]) -> std::result::Result<Id, Error> {
        key_of(presented).and_then(|key| (self.lookup)(&key)).ok_or(Error::InvalidCertificate(CertificateError::ApplicationVerificationFailure))
    }

    fn signed(&self, message: &[u8], key: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> std::result::Result<HandshakeSignatureValid, Error> {
        verify_tls13_signature_with_raw_key(message, &SubjectPublicKeyInfoDer::from(key.as_ref()), dss, &self.algorithms)
    }
}

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(&self, end_entity: &CertificateDer<'_>, _: &[CertificateDer<'_>], _: &ServerName<'_>, _: &[u8], _: UnixTime) -> std::result::Result<ServerCertVerified, Error> {
        self.check(end_entity).map(|_| ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(&self, _: &[u8], _: &CertificateDer<'_>, _: &DigitallySignedStruct) -> std::result::Result<HandshakeSignatureValid, Error> {
        Err(Error::General("a link speaks TLS 1.3 only".into()))
    }

    fn verify_tls13_signature(&self, message: &[u8], key: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> std::result::Result<HandshakeSignatureValid, Error> {
        self.signed(message, key, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SignatureScheme::ED25519]
    }

    fn requires_raw_public_keys(&self) -> bool {
        true
    }
}

impl ClientCertVerifier for Pinned {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(&self, end_entity: &CertificateDer<'_>, _: &[CertificateDer<'_>], _: UnixTime) -> std::result::Result<ClientCertVerified, Error> {
        self.check(end_entity).map(|_| ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(&self, _: &[u8], _: &CertificateDer<'_>, _: &DigitallySignedStruct) -> std::result::Result<HandshakeSignatureValid, Error> {
        Err(Error::General("a link speaks TLS 1.3 only".into()))
    }

    fn verify_tls13_signature(&self, message: &[u8], key: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> std::result::Result<HandshakeSignatureValid, Error> {
        self.signed(message, key, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SignatureScheme::ED25519]
    }

    fn requires_raw_public_keys(&self) -> bool {
        true
    }
}

pub fn client(me: &SecretKeys, peer: Id, peer_key: [u8; 32]) -> Result<Arc<ClientConfig>> {
    let config = ClientConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|error| error.to_string())?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(Pinned::new(fixed(BTreeMap::from([(peer_key, peer)])))))
        .with_client_cert_resolver(Arc::new(rustls::client::AlwaysResolvesClientRawPublicKeys::new(certified(me)?)));
    Ok(Arc::new(config))
}

pub fn server(me: &SecretKeys, allowed: Lookup) -> Result<Arc<ServerConfig>> {
    let config = ServerConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|error| error.to_string())?
        .with_client_cert_verifier(Arc::new(Pinned::new(allowed)))
        .with_cert_resolver(Arc::new(rustls::server::AlwaysResolvesServerRawPublicKeys::new(certified(me)?)));
    Ok(Arc::new(config))
}

pub fn name() -> ServerName<'static> {
    ServerName::try_from("kebbie-link").expect("a fixed DNS name")
}
