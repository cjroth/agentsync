//! Cryptographic services the host supplies to the core: random bytes,
//! signing identities, and TLS cert provisioning.
//!
//! `Rng` is a dependency of the handshake (nonce generation) and identity
//! generation. Native uses `OsRng`; wasm uses `crypto.getRandomValues()` via
//! `web_sys::crypto`.
//!
//! `Signer` abstracts over file-backed identities (sync, in-process) and
//! external signers (ssh-agent on native, WebAuthn / hardware-backed on
//! wasm). `crate::Identity` will implement this directly for the native
//! file-backed path; ssh-agent stays gated to native.
//!
//! `TlsCertProvider` is native-only. Wasm hosts return `None` from
//! `Host::tls()` — TLS termination happens at the underlying transport
//! (browser WebSocket, Node `ws`).

use crate::error::Result;
use crate::identity::{Pubkey, SIGNATURE_LEN};
use async_trait::async_trait;
use std::path::Path;

/// Random byte source. Implementations must be cryptographically secure.
pub trait Rng: Send + Sync + 'static {
    fn fill_bytes(&self, buf: &mut [u8]);
}

/// Anything that can produce ed25519 signatures over a pubkey it claims.
/// The handshake calls `sign` with the canonical transcript bytes; the
/// underlying private key never leaves the signer.
#[async_trait(?Send)]
pub trait Signer: Send + Sync + 'static {
    async fn sign(&self, msg: &[u8]) -> Result<[u8; SIGNATURE_LEN]>;
    fn pubkey(&self) -> Pubkey;
}

/// Native-only. Generates / loads the self-signed TLS cert the hub
/// presents. Browsers never reach this code; their TLS comes from the OS
/// trust store via the WebSocket layer.
#[async_trait(?Send)]
pub trait TlsCertProvider: Send + Sync + 'static {
    /// Load `<dir>/key.der` + `cert.der`, generating a fresh self-signed
    /// pair if absent. The native impl uses rcgen + ed25519.
    async fn load_or_generate(&self, dir: &Path) -> Result<TlsCert>;
}

#[derive(Clone)]
pub struct TlsCert {
    pub cert_der: Vec<u8>,
    pub key_der: Vec<u8>,
}
