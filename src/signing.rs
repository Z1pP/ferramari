//! Ed25519 signing and trust for `.ferramari` packages.
//!
//! Checksums prove a package was not modified after it was built. A signature
//! says *who* built it. The signed message is the raw bytes of
//! `checksums.json`, so one signature covers every file hash and the hashes
//! cover every file.
//!
//! Keys live in the user's config directory:
//!
//! - `signing-key.json` — the local Ed25519 key pair (private key included);
//! - `trusted-keys.json` — public keys the user chose to trust.
//!
//! There is no web of trust and no network validation: an unknown key is
//! reported as unknown and the user decides. Unsigned packages stay importable,
//! because the format shipped before signatures and creators must not be locked
//! out of their own library.

use std::collections::BTreeMap;
use std::fs;
use std::fs::Permissions;
use std::io::Write;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use ring::signature::{Ed25519KeyPair, KeyPair, UnparsedPublicKey, ED25519};
use serde::{Deserialize, Serialize};

use crate::package;
use crate::paths;

pub const SIGNATURE_FILE: &str = "signature.json";
const KEY_FORMAT: u32 = 1;
const MAX_SIGNATURE_BYTES: u64 = 16 * 1024;
const MAX_TRUST_BYTES: u64 = 1024 * 1024;
const PUBLIC_KEY_BYTES: usize = 32;

fn engine() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalKey {
    #[serde(default = "default_format")]
    pub format: u32,
    pub key_id: String,
    pub name: String,
    /// Base64 PKCS#8 v2 document (private + public).
    pub private_pkcs8: String,
    /// Base64 raw 32-byte Ed25519 public key.
    pub public: String,
    pub created: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustedKey {
    pub key_id: String,
    pub name: String,
    /// Base64 raw 32-byte Ed25519 public key.
    pub public: String,
    pub added: u64,
}

impl TrustedKey {
    pub fn from_local(key: &LocalKey) -> Self {
        Self {
            key_id: key.key_id.clone(),
            name: key.name.clone(),
            public: key.public.clone(),
            added: now(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TrustStore {
    format: u32,
    #[serde(default)]
    keys: Vec<TrustedKey>,
}

impl Default for TrustStore {
    fn default() -> Self {
        Self {
            format: KEY_FORMAT,
            keys: Vec::new(),
        }
    }
}

/// Outcome of checking a package's signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignatureStatus {
    /// No `signature.json`; integrity may still be covered by checksums.
    Unsigned,
    /// Signature valid for a key in the trust store.
    Trusted { key_id: String, name: String },
    /// Signature mathematically valid, but the key is not trusted yet.
    Unknown { key_id: String, name: String },
    /// Bad encoding, missing checksums or a signature that does not verify.
    Invalid {
        key_id: Option<String>,
        reason: String,
    },
}

impl SignatureStatus {
    pub fn label(&self) -> String {
        match self {
            SignatureStatus::Unsigned => "unsigned".to_string(),
            SignatureStatus::Trusted { key_id, name } => {
                format!("signed by {name} ({key_id})")
            }
            SignatureStatus::Unknown { key_id, name } => {
                format!("signed by {name}, key {key_id} is not trusted")
            }
            SignatureStatus::Invalid { reason, .. } => format!("invalid signature: {reason}"),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct SignatureFile {
    format: u32,
    algorithm: String,
    key_id: String,
    signature: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    public_key: Option<String>,
}

fn default_format() -> u32 {
    KEY_FORMAT
}

/// Author name recorded on a newly created key.
pub fn default_author_name() -> String {
    std::env::var("USER")
        .ok()
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Ferramari user".to_string())
}

pub fn key_path() -> PathBuf {
    paths::config_dir().join("signing-key.json")
}

pub fn trust_path() -> PathBuf {
    paths::config_dir().join("trusted-keys.json")
}

/// Load the user's signing key, creating one when it does not exist yet.
/// The key is also added to the local trust store: packages built on this
/// machine should verify as trusted without asking the user first.
pub fn load_or_create_key(name: &str) -> Result<LocalKey, String> {
    let key = load_or_create_key_at(&key_path(), name)?;
    if !trusted_keys()
        .iter()
        .any(|trusted| trusted.key_id == key.key_id)
    {
        add_trusted_key(&TrustedKey::from_local(&key))?;
    }
    Ok(key)
}

/// Read an existing key without creating one.
pub fn load_key() -> Result<Option<LocalKey>, String> {
    read_key(&key_path())
}

/// Sign a package when the user already has a key. A missing key is not an
/// error: unsigned packages stay valid, and export must not invent one.
pub fn sign_if_available(root: &Path) -> Result<bool, String> {
    match load_key()? {
        Some(key) => {
            sign_package(root, &key)?;
            Ok(true)
        }
        None => Ok(false),
    }
}

pub fn load_or_create_key_at(path: &Path, name: &str) -> Result<LocalKey, String> {
    if let Some(key) = read_key(path)? {
        return Ok(key);
    }
    let rng = ring::rand::SystemRandom::new();
    let document =
        Ed25519KeyPair::generate_pkcs8(&rng).map_err(|_| "could not generate an Ed25519 key")?;
    let key_pair =
        Ed25519KeyPair::from_pkcs8(document.as_ref()).map_err(|err| format!("bad key: {err}"))?;
    let public = key_pair.public_key().as_ref().to_vec();
    let key = LocalKey {
        format: KEY_FORMAT,
        key_id: key_id(&public),
        name: name.trim().to_string(),
        private_pkcs8: engine().encode(document.as_ref()),
        public: engine().encode(&public),
        created: now(),
    };
    write_json(path, &key, 0o600)?;
    log::info!("[Signing] Created key {} ({})", key.name, key.key_id);
    Ok(key)
}

fn read_key(path: &Path) -> Result<Option<LocalKey>, String> {
    if !path.is_file() {
        return Ok(None);
    }
    let text =
        fs::read_to_string(path).map_err(|err| format!("cannot read {}: {err}", path.display()))?;
    let key: LocalKey =
        serde_json::from_str(&text).map_err(|err| format!("invalid signing key: {err}"))?;
    if key.format != KEY_FORMAT {
        return Err(format!("unsupported signing key format {}", key.format));
    }
    Ok(Some(key))
}

impl LocalKey {
    fn key_pair(&self) -> Result<Ed25519KeyPair, String> {
        let pkcs8 = engine()
            .decode(&self.private_pkcs8)
            .map_err(|err| format!("corrupt private key: {err}"))?;
        Ed25519KeyPair::from_pkcs8(&pkcs8).map_err(|err| format!("cannot open key: {err}"))
    }
}

/// Write `signature.json` next to the package's `checksums.json`. The file also
/// carries the public key and author name, so a recipient can verify a package
/// before deciding whether to trust the key.
pub fn sign_package(root: &Path, key: &LocalKey) -> Result<(), String> {
    let message = fs::read(root.join(package::CHECKSUMS_FILE))
        .map_err(|_| "signing requires checksums.json".to_string())?;
    let signature = key.key_pair()?.sign(&message).as_ref().to_vec();
    let file = SignatureFile {
        format: KEY_FORMAT,
        algorithm: "ed25519".to_string(),
        key_id: key.key_id.clone(),
        signature: engine().encode(signature),
        name: Some(key.name.clone()),
        public_key: Some(key.public.clone()),
    };
    let text = serde_json::to_string_pretty(&file)
        .map_err(|err| format!("cannot serialize the signature: {err}"))?;
    write_text(&root.join(SIGNATURE_FILE), &text)
}

/// Check the package signature against the trust store.
pub fn verify_package(root: &Path) -> Result<SignatureStatus, String> {
    // An unsigned package must not read the trust store at all.
    if !root.join(SIGNATURE_FILE).is_file() {
        return Ok(SignatureStatus::Unsigned);
    }
    verify_package_with_trust(root, &trusted_keys())
}

fn verify_package_with_trust(
    root: &Path,
    trusted: &[TrustedKey],
) -> Result<SignatureStatus, String> {
    let path = root.join(SIGNATURE_FILE);
    if !path.is_file() {
        return Ok(SignatureStatus::Unsigned);
    }
    let metadata =
        fs::metadata(&path).map_err(|err| format!("cannot stat {}: {err}", path.display()))?;
    if metadata.len() > MAX_SIGNATURE_BYTES {
        return Ok(invalid(None, "signature.json is too large".to_string()));
    }
    let text = fs::read_to_string(&path).map_err(|err| format!("cannot read signature: {err}"))?;
    let file: SignatureFile = match serde_json::from_str(&text) {
        Ok(file) => file,
        Err(err) => return Ok(invalid(None, format!("invalid signature.json: {err}"))),
    };
    if file.format != KEY_FORMAT {
        return Ok(invalid(
            Some(file.key_id),
            format!("unsupported signature format {}", file.format),
        ));
    }
    if file.algorithm != "ed25519" {
        return Ok(invalid(
            Some(file.key_id),
            format!("unsupported algorithm {}", file.algorithm),
        ));
    }
    let Ok(message) = fs::read(root.join(package::CHECKSUMS_FILE)) else {
        return Ok(invalid(
            Some(file.key_id),
            "checksums.json is missing".to_string(),
        ));
    };
    let Ok(signature) = engine().decode(&file.signature) else {
        return Ok(invalid(
            Some(file.key_id),
            "corrupt signature encoding".to_string(),
        ));
    };

    for candidate in trusted {
        if candidate.key_id != file.key_id {
            continue;
        }
        let Some(public) = decode_public_key(&candidate.public) else {
            continue;
        };
        if UnparsedPublicKey::new(&ED25519, &public)
            .verify(&message, &signature)
            .is_ok()
        {
            return Ok(SignatureStatus::Trusted {
                key_id: candidate.key_id.clone(),
                name: candidate.name.clone(),
            });
        }
    }

    // Not trusted: the package may still carry the author's key, which lets us
    // report "verified signature, unknown author" instead of "cannot check".
    match file.public_key.as_deref().and_then(decode_public_key) {
        Some(public) if key_id(&public) == file.key_id => {
            if UnparsedPublicKey::new(&ED25519, &public)
                .verify(&message, &signature)
                .is_ok()
            {
                Ok(SignatureStatus::Unknown {
                    key_id: file.key_id,
                    name: file.name.unwrap_or_else(|| "Unknown creator".to_string()),
                })
            } else {
                Ok(invalid(
                    Some(file.key_id),
                    "signature does not verify".to_string(),
                ))
            }
        }
        Some(_) => Ok(invalid(
            Some(file.key_id),
            "the embedded key does not match the key id".to_string(),
        )),
        None => Ok(invalid(
            Some(file.key_id),
            "the key is not trusted and no public key is embedded".to_string(),
        )),
    }
}

fn invalid(key_id: Option<String>, reason: String) -> SignatureStatus {
    SignatureStatus::Invalid { key_id, reason }
}

fn decode_public_key(encoded: &str) -> Option<Vec<u8>> {
    let public = engine().decode(encoded).ok()?;
    (public.len() == PUBLIC_KEY_BYTES).then_some(public)
}

/// Trust the author's key when the package embeds one and its signature checks.
pub fn trust_key_embedded_in(root: &Path) -> Result<Option<TrustedKey>, String> {
    trust_key_embedded_in_at(root, &trust_path())
}

fn trust_key_embedded_in_at(root: &Path, trust: &Path) -> Result<Option<TrustedKey>, String> {
    let Ok(text) = fs::read_to_string(root.join(SIGNATURE_FILE)) else {
        return Ok(None);
    };
    let file: SignatureFile =
        serde_json::from_str(&text).map_err(|err| format!("invalid signature: {err}"))?;
    let Some(public) = file.public_key.as_deref().and_then(decode_public_key) else {
        return Ok(None);
    };
    if key_id(&public) != file.key_id {
        return Err("the embedded key does not match the key id".to_string());
    }
    let message =
        fs::read(root.join(package::CHECKSUMS_FILE)).map_err(|_| "checksums.json is missing")?;
    let signature = engine()
        .decode(&file.signature)
        .map_err(|_| "corrupt signature encoding".to_string())?;
    if UnparsedPublicKey::new(&ED25519, &public)
        .verify(&message, &signature)
        .is_err()
    {
        return Err("the package signature does not verify".to_string());
    }
    let key = TrustedKey {
        key_id: file.key_id,
        name: file.name.unwrap_or_else(|| "Unknown creator".to_string()),
        public: file.public_key.unwrap_or_default(),
        added: now(),
    };
    add_trusted_key_at(&key, trust)?;
    Ok(Some(key))
}

/// Trust a key from either a signed package directory or a public key file.
/// Returns the key id when something was trusted.
pub fn trust_source(path: &Path) -> Result<Option<String>, String> {
    if path.join(SIGNATURE_FILE).is_file() {
        return trust_key_embedded_in(path).map(|key| key.map(|key| key.key_id));
    }
    let key = read_public_key_document(path)?;
    let key_id = key.key_id.clone();
    add_trusted_key(&key).map(|()| Some(key_id))
}

pub fn trusted_keys() -> Vec<TrustedKey> {
    read_trust_store(&trust_path())
        .map(|store| store.keys)
        .unwrap_or_default()
}

fn read_trust_store(path: &Path) -> Result<TrustStore, String> {
    if !path.is_file() {
        return Ok(TrustStore::default());
    }
    let metadata =
        fs::metadata(path).map_err(|err| format!("cannot stat {}: {err}", path.display()))?;
    if metadata.len() > MAX_TRUST_BYTES {
        return Err("trusted-keys.json is too large".to_string());
    }
    let text =
        fs::read_to_string(path).map_err(|err| format!("cannot read {}: {err}", path.display()))?;
    let store: TrustStore =
        serde_json::from_str(&text).map_err(|err| format!("invalid trusted-keys.json: {err}"))?;
    if store.format != KEY_FORMAT {
        return Err(format!("unsupported trust store format {}", store.format));
    }
    Ok(store)
}

pub fn add_trusted_key(key: &TrustedKey) -> Result<(), String> {
    add_trusted_key_at(key, &trust_path())
}

fn add_trusted_key_at(key: &TrustedKey, path: &Path) -> Result<(), String> {
    if key.key_id.is_empty() {
        return Err("a trusted key needs a key id".to_string());
    }
    if decode_public_key(&key.public).is_none() {
        return Err("a trusted key needs a 32-byte public key".to_string());
    }
    let mut store = read_trust_store(path)?;
    if let Some(existing) = store
        .keys
        .iter_mut()
        .find(|existing| existing.key_id == key.key_id)
    {
        existing.name = key.name.clone();
        existing.public = key.public.clone();
    } else {
        store.keys.push(key.clone());
    }
    store.keys.sort_by(|a, b| a.name.cmp(&b.name));
    write_json(path, &store, 0o644)
}

/// Public half of the local key, ready to hand to other users.
pub fn public_key_document(key: &LocalKey) -> String {
    let document = BTreeMap::from([
        ("format".to_string(), serde_json::json!(KEY_FORMAT)),
        ("key_id".to_string(), serde_json::json!(key.key_id)),
        ("name".to_string(), serde_json::json!(key.name)),
        ("public_key".to_string(), serde_json::json!(key.public)),
    ]);
    serde_json::to_string_pretty(&document).unwrap_or_default()
}

pub fn read_public_key_document(path: &Path) -> Result<TrustedKey, String> {
    let text =
        fs::read_to_string(path).map_err(|err| format!("cannot read {}: {err}", path.display()))?;
    let value: BTreeMap<String, serde_json::Value> =
        serde_json::from_str(&text).map_err(|err| format!("invalid key file: {err}"))?;
    let encoded = value
        .get("public_key")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "the key file has no public_key".to_string())?;
    let public = decode_public_key(encoded)
        .ok_or_else(|| "an Ed25519 public key must be 32 bytes".to_string())?;
    Ok(TrustedKey {
        key_id: key_id(&public),
        name: value
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("Unnamed creator")
            .trim()
            .to_string(),
        public: encoded.to_string(),
        added: now(),
    })
}

fn key_id(public: &[u8]) -> String {
    let Some(mut checksum) = glib::Checksum::new(glib::ChecksumType::Sha256) else {
        return "no-sha256".to_string();
    };
    checksum.update(public);
    let digest = checksum.string().unwrap_or_default();
    digest.chars().take(16).collect()
}

fn write_json<T: Serialize>(path: &Path, value: &T, mode: u32) -> Result<(), String> {
    let text = serde_json::to_string_pretty(value)
        .map_err(|err| format!("cannot serialize {}: {err}", path.display()))?;
    write_text(path, &text)?;
    let _ = fs::set_permissions(path, Permissions::from_mode(mode));
    Ok(())
}

fn write_text(path: &Path, text: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let temp = path.with_extension("tmp");
    {
        let mut file = fs::File::create(&temp)
            .map_err(|err| format!("cannot write {}: {err}", path.display()))?;
        file.write_all(format!("{text}\n").as_bytes())
            .map_err(|err| format!("cannot write {}: {err}", path.display()))?;
        file.sync_all()
            .map_err(|err| format!("cannot flush {}: {err}", path.display()))?;
    }
    fs::rename(&temp, path).map_err(|err| format!("cannot store {}: {err}", path.display()))?;
    Ok(())
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "ferramari-signing-{name}-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn key() -> LocalKey {
        let rng = ring::rand::SystemRandom::new();
        let document = Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
        let key_pair = Ed25519KeyPair::from_pkcs8(document.as_ref()).unwrap();
        let public = key_pair.public_key().as_ref().to_vec();
        LocalKey {
            format: KEY_FORMAT,
            key_id: key_id(&public),
            name: "Test Creator".to_string(),
            private_pkcs8: engine().encode(document.as_ref()),
            public: engine().encode(&public),
            created: now(),
        }
    }

    fn package_dir(name: &str) -> PathBuf {
        let dir = temp_dir(name);
        fs::write(
            dir.join("manifest.json"),
            r#"{"name":"Signed","author":"A","version":"1.0","type":"video"}"#,
        )
        .unwrap();
        fs::write(dir.join("wallpaper.mp4"), b"video bytes").unwrap();
        package::write_checksums(&dir).unwrap();
        dir
    }

    #[test]
    fn sign_then_verify_reports_an_unknown_key() {
        let dir = package_dir("unknown");
        let key = key();
        sign_package(&dir, &key).unwrap();
        assert_eq!(
            verify_package_with_trust(&dir, &[]).unwrap(),
            SignatureStatus::Unknown {
                key_id: key.key_id.clone(),
                name: "Test Creator".to_string(),
            }
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_trusted_key_verifies_the_package() {
        let dir = package_dir("trusted");
        let key = key();
        sign_package(&dir, &key).unwrap();
        let trusted = vec![TrustedKey::from_local(&key)];
        assert_eq!(
            verify_package_with_trust(&dir, &trusted).unwrap(),
            SignatureStatus::Trusted {
                key_id: key.key_id.clone(),
                name: "Test Creator".to_string(),
            }
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn trusting_the_embedded_key_makes_the_package_trusted() {
        let dir = package_dir("trust");
        let key = key();
        sign_package(&dir, &key).unwrap();
        let trust = temp_dir("trust-store").join("trusted-keys.json");
        let added = trust_key_embedded_in_at(&dir, &trust).unwrap().unwrap();
        assert_eq!(added.key_id, key.key_id);
        let store = read_trust_store(&trust).unwrap();
        assert_eq!(
            verify_package_with_trust(&dir, &store.keys).unwrap(),
            SignatureStatus::Trusted {
                key_id: key.key_id.clone(),
                name: "Test Creator".to_string(),
            }
        );
        let _ = fs::remove_dir_all(dir);
        let _ = fs::remove_dir_all(trust.parent().unwrap());
    }

    #[test]
    fn a_modified_file_is_caught_by_the_checksums() {
        let dir = package_dir("tampered");
        let key = key();
        sign_package(&dir, &key).unwrap();
        let trusted = vec![TrustedKey::from_local(&key)];
        fs::write(dir.join("wallpaper.mp4"), b"other bytes").unwrap();
        // The signature covers `checksums.json`, so it still verifies; the file
        // hash no longer matches, which is what rejects the package.
        assert!(matches!(
            verify_package_with_trust(&dir, &trusted).unwrap(),
            SignatureStatus::Trusted { .. }
        ));
        let error = package::load(&dir).unwrap_err();
        assert!(error.contains("SHA-256"), "unexpected error: {error}");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn editing_the_checksums_breaks_the_signature() {
        let dir = package_dir("recut");
        let key = key();
        sign_package(&dir, &key).unwrap();
        let trusted = vec![TrustedKey::from_local(&key)];
        let path = dir.join(package::CHECKSUMS_FILE);
        let mut text = fs::read_to_string(&path).unwrap();
        text = text.replace("wallpaper.mp4", "other.mp4");
        fs::write(&path, text).unwrap();
        assert!(matches!(
            verify_package_with_trust(&dir, &trusted).unwrap(),
            SignatureStatus::Invalid { .. }
        ));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn unsigned_packages_are_reported_not_rejected() {
        let dir = package_dir("unsigned");
        assert_eq!(
            verify_package_with_trust(&dir, &[]).unwrap(),
            SignatureStatus::Unsigned
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_signature_with_a_replaced_key_cannot_be_verified() {
        let dir = package_dir("nokey");
        let key = key();
        sign_package(&dir, &key).unwrap();
        let path = dir.join(SIGNATURE_FILE);
        let text = fs::read_to_string(&path).unwrap();
        let mut value: BTreeMap<String, serde_json::Value> = serde_json::from_str(&text).unwrap();
        value.insert(
            "public_key".to_string(),
            serde_json::Value::String(engine().encode([0u8; PUBLIC_KEY_BYTES])),
        );
        fs::write(&path, serde_json::to_string(&value).unwrap()).unwrap();
        assert!(matches!(
            verify_package_with_trust(&dir, &[]).unwrap(),
            SignatureStatus::Invalid { .. }
        ));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn public_key_documents_roundtrip() {
        let key = key();
        let dir = temp_dir("pubkey");
        let path = dir.join("creator.json");
        fs::write(&path, public_key_document(&key)).unwrap();
        let read = read_public_key_document(&path).unwrap();
        assert_eq!(read.key_id, key.key_id);
        assert_eq!(read.name, "Test Creator");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_key_file_with_a_short_key_is_rejected() {
        let dir = temp_dir("shortkey");
        let path = dir.join("bad.json");
        fs::write(
            &path,
            format!(
                r#"{{"format":1,"key_id":"x","name":"Bad","public_key":"{}"}}"#,
                engine().encode([0u8; 8])
            ),
        )
        .unwrap();
        assert!(read_public_key_document(&path).is_err());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn the_local_key_is_created_once_and_kept() {
        let dir = temp_dir("localkey");
        let path = dir.join("signing-key.json");
        let first = load_or_create_key_at(&path, "Me").unwrap();
        let second = load_or_create_key_at(&path, "Someone else").unwrap();
        assert_eq!(first.key_id, second.key_id);
        assert_eq!(second.name, "Me");
        assert!(path.is_file());
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o077;
        assert_eq!(mode, 0o0, "the private key must not be group or world");
        let _ = fs::remove_dir_all(dir);
    }
}
