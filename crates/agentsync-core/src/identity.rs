//! Per-peer ed25519 identity keys, modeled after SSH.
//!
//! An [`Identity`] is the device-local secret used to sign handshake
//! transcripts. Two backends are supported:
//!
//! - **File**: a 32-byte ed25519 seed stored on disk (mode 0600). Generated
//!   by `agentsync init` / `agentsync key generate`.
//! - **Agent**: signing is delegated to an ssh-agent-protocol socket.
//!   Suitable for hardware-backed keys (Secretive, 1Password, ssh-agent,
//!   YubiKey-Agent, gpg-agent).
//!
//! Both expose the same async `sign` interface; the rest of the codebase
//! does not care which backend is in use.

use crate::error::{Error, Result};
use base64::Engine;
use ed25519_dalek::{Signer, SigningKey, Verifier, VerifyingKey, SECRET_KEY_LENGTH};
use rand_core::OsRng;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub const PUBKEY_LEN: usize = 32;
pub const SIGNATURE_LEN: usize = 64;
const SSH_KEY_TYPE: &str = "ssh-ed25519";

const SSH_AGENTC_REQUEST_IDENTITIES: u8 = 11;
const SSH_AGENT_IDENTITIES_ANSWER: u8 = 12;
const SSH_AGENTC_SIGN_REQUEST: u8 = 13;
const SSH_AGENT_SIGN_RESPONSE: u8 = 14;
const SSH_AGENT_FAILURE: u8 = 5;

/// Identity used to authenticate the local peer in handshakes.
#[derive(Clone)]
pub enum Identity {
    /// Secret-key bytes held in process memory.
    File { signing: SigningKey, pubkey: Pubkey },
    /// Signing delegated to an ssh-agent over a Unix socket.
    Agent { socket: PathBuf, pubkey: Pubkey },
}

impl Identity {
    /// Generate a fresh file-backed identity.
    pub fn generate() -> Self {
        let signing = SigningKey::generate(&mut OsRng);
        let pubkey = Pubkey(signing.verifying_key().to_bytes());
        Self::File { signing, pubkey }
    }

    pub fn from_seed(seed: [u8; SECRET_KEY_LENGTH]) -> Self {
        let signing = SigningKey::from_bytes(&seed);
        let pubkey = Pubkey(signing.verifying_key().to_bytes());
        Self::File { signing, pubkey }
    }

    /// Build an agent-backed identity. The pubkey selects which key the
    /// agent should sign with (an agent often holds many).
    pub fn from_agent(socket: PathBuf, pubkey: Pubkey) -> Self {
        Self::Agent { socket, pubkey }
    }

    pub fn pubkey(&self) -> Pubkey {
        match self {
            Self::File { pubkey, .. } => *pubkey,
            Self::Agent { pubkey, .. } => *pubkey,
        }
    }

    /// File-backed only: returns the 32-byte seed. Agent-backed identities
    /// have no exportable secret on this side and return an error.
    pub fn seed(&self) -> Result<[u8; SECRET_KEY_LENGTH]> {
        match self {
            Self::File { signing, .. } => Ok(signing.to_bytes()),
            Self::Agent { .. } => Err(Error::Auth(
                "agent-backed identity has no exportable seed".into(),
            )),
        }
    }

    /// Sign a message. For file-backed identities this is local. For
    /// agent-backed identities this round-trips through the agent socket.
    pub async fn sign(&self, message: &[u8]) -> Result<[u8; SIGNATURE_LEN]> {
        match self {
            Self::File { signing, .. } => Ok(signing.sign(message).to_bytes()),
            Self::Agent { socket, pubkey } => agent_sign(socket, pubkey, message).await,
        }
    }

    /// Persist a file-backed identity to `path` as a single-line base64 of
    /// the seed. Errors for agent-backed identities.
    pub fn save_to_file(&self, path: &Path) -> Result<()> {
        let seed = self.seed()?;
        let seed_b64 =
            base64::engine::general_purpose::STANDARD_NO_PAD.encode(seed);
        let body = format!("agentsync-identity-v1 {}\n", seed_b64);
        write_with_mode(path, body.as_bytes(), 0o600)?;
        let pub_path = pubkey_sidecar(path);
        std::fs::write(&pub_path, format!("{}\n", self.pubkey().to_ssh_string()))?;
        Ok(())
    }

    pub fn load_from_file(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path)?;
        let s = std::str::from_utf8(&bytes).map_err(|_| Error::InvalidUtf8)?;
        let line = s.lines().next().ok_or_else(|| {
            Error::Auth(format!("empty identity file at {}", path.display()))
        })?;
        let rest = line
            .strip_prefix("agentsync-identity-v1 ")
            .ok_or_else(|| {
                Error::Auth(format!(
                    "identity file at {} is not in agentsync-identity-v1 format",
                    path.display()
                ))
            })?
            .trim();
        let seed_bytes = base64::engine::general_purpose::STANDARD_NO_PAD
            .decode(rest)
            .map_err(|e| Error::Auth(format!("identity file base64 decode: {}", e)))?;
        if seed_bytes.len() != SECRET_KEY_LENGTH {
            return Err(Error::Auth(format!(
                "identity seed wrong length: got {} bytes, want {}",
                seed_bytes.len(),
                SECRET_KEY_LENGTH
            )));
        }
        let mut seed = [0u8; SECRET_KEY_LENGTH];
        seed.copy_from_slice(&seed_bytes);
        Ok(Self::from_seed(seed))
    }
}

impl std::fmt::Debug for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::File { pubkey, .. } => write!(f, "Identity::File({})", pubkey.to_ssh_string()),
            Self::Agent { socket, pubkey } => write!(
                f,
                "Identity::Agent({} via {})",
                pubkey.to_ssh_string(),
                socket.display()
            ),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Pubkey(pub [u8; PUBKEY_LEN]);

impl Pubkey {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != PUBKEY_LEN {
            return Err(Error::Auth(format!(
                "pubkey wrong length: got {} bytes, want {}",
                bytes.len(),
                PUBKEY_LEN
            )));
        }
        let mut out = [0u8; PUBKEY_LEN];
        out.copy_from_slice(bytes);
        Ok(Self(out))
    }

    pub fn as_bytes(&self) -> &[u8; PUBKEY_LEN] {
        &self.0
    }

    pub fn to_ssh_string(&self) -> String {
        let wire = encode_ssh_wire(&self.0);
        let b64 = base64::engine::general_purpose::STANDARD.encode(wire);
        format!("{} {}", SSH_KEY_TYPE, b64)
    }

    pub fn from_ssh_string(s: &str) -> Result<Self> {
        let trimmed = s.trim();
        let mut parts = trimmed.split_whitespace();
        let kind = parts
            .next()
            .ok_or_else(|| Error::Auth("empty ssh pubkey string".into()))?;
        if kind != SSH_KEY_TYPE {
            return Err(Error::Auth(format!(
                "unsupported pubkey type {:?}, want {}",
                kind, SSH_KEY_TYPE
            )));
        }
        let b64 = parts
            .next()
            .ok_or_else(|| Error::Auth("missing base64 portion of ssh pubkey".into()))?;
        let wire = base64::engine::general_purpose::STANDARD
            .decode(b64.as_bytes())
            .map_err(|e| Error::Auth(format!("ssh pubkey base64: {}", e)))?;
        let raw = decode_ssh_wire(&wire)?;
        Ok(Self(raw))
    }

    pub fn fingerprint_sha256(&self) -> String {
        let wire = encode_ssh_wire(&self.0);
        let mut hasher = Sha256::new();
        hasher.update(&wire);
        let digest = hasher.finalize();
        let b64 = base64::engine::general_purpose::STANDARD_NO_PAD.encode(digest);
        format!("SHA256:{}", b64)
    }

    pub fn verify(&self, message: &[u8], signature: &[u8]) -> bool {
        if signature.len() != SIGNATURE_LEN {
            return false;
        }
        let vk = match VerifyingKey::from_bytes(&self.0) {
            Ok(v) => v,
            Err(_) => return false,
        };
        let mut sig_bytes = [0u8; SIGNATURE_LEN];
        sig_bytes.copy_from_slice(signature);
        let sig = ed25519_dalek::Signature::from_bytes(&sig_bytes);
        vk.verify(message, &sig).is_ok()
    }

    /// SSH wire encoding of this pubkey (4-byte-length prefixed strings):
    /// `string("ssh-ed25519") || string(<32 bytes>)`.
    pub fn to_ssh_wire(&self) -> Vec<u8> {
        encode_ssh_wire(&self.0)
    }
}

impl std::fmt::Debug for Pubkey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Pubkey({})", self.to_ssh_string())
    }
}

impl std::fmt::Display for Pubkey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_ssh_string())
    }
}

fn encode_ssh_wire(pubkey: &[u8; PUBKEY_LEN]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + SSH_KEY_TYPE.len() + 4 + PUBKEY_LEN);
    out.extend_from_slice(&(SSH_KEY_TYPE.len() as u32).to_be_bytes());
    out.extend_from_slice(SSH_KEY_TYPE.as_bytes());
    out.extend_from_slice(&(PUBKEY_LEN as u32).to_be_bytes());
    out.extend_from_slice(pubkey);
    out
}

fn decode_ssh_wire(wire: &[u8]) -> Result<[u8; PUBKEY_LEN]> {
    let mut cursor = 0usize;
    let kind = read_ssh_string(wire, &mut cursor)?;
    if kind != SSH_KEY_TYPE.as_bytes() {
        return Err(Error::Auth(format!(
            "ssh pubkey type tag mismatch: got {:?}",
            String::from_utf8_lossy(kind)
        )));
    }
    let pubkey_bytes = read_ssh_string(wire, &mut cursor)?;
    if pubkey_bytes.len() != PUBKEY_LEN {
        return Err(Error::Auth(format!(
            "ssh pubkey body wrong length: got {}, want {}",
            pubkey_bytes.len(),
            PUBKEY_LEN
        )));
    }
    let mut out = [0u8; PUBKEY_LEN];
    out.copy_from_slice(pubkey_bytes);
    Ok(out)
}

fn read_ssh_string<'a>(buf: &'a [u8], cursor: &mut usize) -> Result<&'a [u8]> {
    if buf.len() < *cursor + 4 {
        return Err(Error::Auth("truncated ssh wire string length".into()));
    }
    let len = u32::from_be_bytes([
        buf[*cursor],
        buf[*cursor + 1],
        buf[*cursor + 2],
        buf[*cursor + 3],
    ]) as usize;
    *cursor += 4;
    if buf.len() < *cursor + len {
        return Err(Error::Auth("truncated ssh wire string body".into()));
    }
    let s = &buf[*cursor..*cursor + len];
    *cursor += len;
    Ok(s)
}

fn pubkey_sidecar(secret_path: &Path) -> std::path::PathBuf {
    match secret_path.extension() {
        Some(_) => {
            let mut p = secret_path.to_path_buf();
            p.set_extension(format!(
                "{}.pub",
                secret_path
                    .extension()
                    .map(|e| e.to_string_lossy().into_owned())
                    .unwrap_or_default()
            ));
            p
        }
        None => secret_path.with_extension("pub"),
    }
}

#[cfg(unix)]
fn write_with_mode(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(mode)
        .open(path)?;
    use std::io::Write;
    f.write_all(bytes)?;
    Ok(())
}

#[cfg(not(unix))]
fn write_with_mode(path: &Path, bytes: &[u8], _mode: u32) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, bytes)?;
    Ok(())
}

// ---- ssh-agent protocol ----

/// Ask the agent at `socket` to sign `message` with the key identified by
/// `pubkey`. Returns the raw 64-byte ed25519 signature.
async fn agent_sign(socket: &Path, pubkey: &Pubkey, message: &[u8]) -> Result<[u8; SIGNATURE_LEN]> {
    #[cfg(unix)]
    {
        use tokio::net::UnixStream;
        let mut stream = UnixStream::connect(socket).await.map_err(|e| {
            Error::Auth(format!(
                "ssh-agent connect to {}: {}",
                socket.display(),
                e
            ))
        })?;

        // Sanity: make sure the agent actually holds this key. Catches the
        // common "wrong agent socket" misconfiguration with a clear error
        // before we ask it to sign.
        let identities = agent_list_identities(&mut stream).await?;
        if !identities.contains(pubkey) {
            return Err(Error::Auth(format!(
                "ssh-agent at {} has no key matching {}",
                socket.display(),
                pubkey.fingerprint_sha256()
            )));
        }

        // Build SSH_AGENTC_SIGN_REQUEST.
        let mut payload = Vec::new();
        payload.push(SSH_AGENTC_SIGN_REQUEST);
        write_string(&mut payload, &pubkey.to_ssh_wire());
        write_string(&mut payload, message);
        payload.extend_from_slice(&0u32.to_be_bytes()); // flags
        let mut req = Vec::new();
        req.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        req.extend_from_slice(&payload);
        stream
            .write_all(&req)
            .await
            .map_err(|e| Error::Auth(format!("ssh-agent write: {}", e)))?;

        let resp = read_message(&mut stream).await?;
        if resp.is_empty() {
            return Err(Error::Auth("ssh-agent empty response".into()));
        }
        match resp[0] {
            SSH_AGENT_SIGN_RESPONSE => {
                // payload[1..] = string<sig_blob>
                let mut cursor = 1usize;
                let sig_blob = read_string(&resp, &mut cursor)?;
                // sig_blob = string("ssh-ed25519") || string(<64 bytes>)
                let mut inner = 0usize;
                let kind = read_string(sig_blob, &mut inner)?;
                if kind != SSH_KEY_TYPE.as_bytes() {
                    return Err(Error::Auth(format!(
                        "ssh-agent returned unexpected key type {:?}",
                        String::from_utf8_lossy(kind)
                    )));
                }
                let sig_bytes = read_string(sig_blob, &mut inner)?;
                if sig_bytes.len() != SIGNATURE_LEN {
                    return Err(Error::Auth(format!(
                        "ssh-agent returned signature of wrong length: {}",
                        sig_bytes.len()
                    )));
                }
                let mut out = [0u8; SIGNATURE_LEN];
                out.copy_from_slice(sig_bytes);
                Ok(out)
            }
            SSH_AGENT_FAILURE => Err(Error::Auth(
                "ssh-agent refused to sign (user cancelled, key locked, or wrong key)"
                    .into(),
            )),
            other => Err(Error::Auth(format!(
                "ssh-agent returned unexpected response type {}",
                other
            ))),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (socket, pubkey, message);
        Err(Error::Auth(
            "ssh-agent backend is only available on Unix".into(),
        ))
    }
}

/// List all ed25519 pubkeys held by the agent. Other key types (RSA, ECDSA)
/// are ignored — agentsync only signs with ed25519.
pub async fn agent_list_identities_at(socket: &Path) -> Result<Vec<Pubkey>> {
    #[cfg(unix)]
    {
        use tokio::net::UnixStream;
        let mut stream = UnixStream::connect(socket).await.map_err(|e| {
            Error::Auth(format!(
                "ssh-agent connect to {}: {}",
                socket.display(),
                e
            ))
        })?;
        agent_list_identities(&mut stream).await
    }
    #[cfg(not(unix))]
    {
        let _ = socket;
        Err(Error::Auth(
            "ssh-agent backend is only available on Unix".into(),
        ))
    }
}

#[cfg(unix)]
async fn agent_list_identities(stream: &mut tokio::net::UnixStream) -> Result<Vec<Pubkey>> {
    let req = vec![0, 0, 0, 1, SSH_AGENTC_REQUEST_IDENTITIES];
    stream
        .write_all(&req)
        .await
        .map_err(|e| Error::Auth(format!("ssh-agent write: {}", e)))?;
    let resp = read_message(stream).await?;
    if resp.is_empty() || resp[0] != SSH_AGENT_IDENTITIES_ANSWER {
        return Err(Error::Auth("ssh-agent did not return identities".into()));
    }
    let mut cursor = 1usize;
    if resp.len() < cursor + 4 {
        return Err(Error::Auth("ssh-agent identities response truncated".into()));
    }
    let n = u32::from_be_bytes([
        resp[cursor],
        resp[cursor + 1],
        resp[cursor + 2],
        resp[cursor + 3],
    ]) as usize;
    cursor += 4;
    let mut keys = Vec::new();
    for _ in 0..n {
        let blob = read_string(&resp, &mut cursor)?;
        let _comment = read_string(&resp, &mut cursor)?;
        // Each blob is the SSH wire encoding of one pubkey. We only handle
        // ssh-ed25519; other types are silently skipped.
        if let Ok(raw) = decode_ssh_wire(blob) {
            keys.push(Pubkey(raw));
        }
    }
    Ok(keys)
}

#[cfg(unix)]
async fn read_message(stream: &mut tokio::net::UnixStream) -> Result<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    stream
        .read_exact(&mut len_buf)
        .await
        .map_err(|e| Error::Auth(format!("ssh-agent read len: {}", e)))?;
    let len = u32::from_be_bytes(len_buf) as usize;
    let mut buf = vec![0u8; len];
    stream
        .read_exact(&mut buf)
        .await
        .map_err(|e| Error::Auth(format!("ssh-agent read body: {}", e)))?;
    Ok(buf)
}

fn write_string(out: &mut Vec<u8>, s: &[u8]) {
    out.extend_from_slice(&(s.len() as u32).to_be_bytes());
    out.extend_from_slice(s);
}

fn read_string<'a>(buf: &'a [u8], cursor: &mut usize) -> Result<&'a [u8]> {
    read_ssh_string(buf, cursor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn sign_verify_round_trip_file() {
        let id = Identity::generate();
        let pk = id.pubkey();
        let msg = b"hello";
        let sig = id.sign(msg).await.unwrap();
        assert!(pk.verify(msg, &sig));
        assert!(!pk.verify(b"hello!", &sig));
    }

    #[test]
    fn pubkey_ssh_round_trip() {
        let id = Identity::generate();
        let s = id.pubkey().to_ssh_string();
        assert!(s.starts_with("ssh-ed25519 "));
        let pk = Pubkey::from_ssh_string(&s).unwrap();
        assert_eq!(pk, id.pubkey());
    }

    #[test]
    fn pubkey_rejects_wrong_type() {
        let err = Pubkey::from_ssh_string("ssh-rsa AAAA").unwrap_err();
        assert!(matches!(err, Error::Auth(_)));
    }

    #[test]
    fn fingerprint_format() {
        let id = Identity::generate();
        let fp = id.pubkey().fingerprint_sha256();
        assert!(fp.starts_with("SHA256:"));
        assert_eq!(fp.len(), 50);
    }

    #[test]
    fn save_load_round_trip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("identity");
        let id = Identity::generate();
        id.save_to_file(&path).unwrap();
        let loaded = Identity::load_from_file(&path).unwrap();
        assert_eq!(loaded.pubkey(), id.pubkey());
    }

    #[test]
    fn agent_identity_has_no_seed() {
        let id = Identity::from_agent(
            std::path::PathBuf::from("/tmp/no-such.sock"),
            Identity::generate().pubkey(),
        );
        assert!(id.seed().is_err());
    }
}
