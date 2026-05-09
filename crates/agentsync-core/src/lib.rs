//! agentsync-core — real-time directory sync engine using Automerge CRDTs.
//!
//! See [`SPEC.md`] in the repo root for the full design. On native targets,
//! the high-level API lives on [`Vault`]. On `wasm32-unknown-unknown`, only
//! the wasm-safe subset compiles: CRDT primitives, identity (file-backed),
//! authorized_keys parsing, the protocol Frame codec, and handshake helpers.
//! Tokio sockets, rustls, the `notify` file watcher, and on-disk stores are
//! gated to native builds via `cfg(not(target_arch = "wasm32"))`.

pub mod auth;
pub mod constants;
pub mod doc;
pub mod error;
pub mod identity;
pub mod net;
pub mod path;
pub mod peers_md;

#[cfg(not(target_arch = "wasm32"))]
pub mod fs;
#[cfg(not(target_arch = "wasm32"))]
pub mod host;
#[cfg(not(target_arch = "wasm32"))]
pub mod store;
#[cfg(not(target_arch = "wasm32"))]
pub mod tls;
#[cfg(not(target_arch = "wasm32"))]
pub mod vault;

pub use auth::{HANDSHAKE_DOMAIN, NONCE_LEN, build_transcript, random_nonce};
pub use constants::{
    AUTHORIZED_KEYS_FILE, DEFAULT_LISTEN_ADDR, DEFAULT_LISTEN_ADDR_NO_TLS, DEFAULT_PORT,
    USER_IDENTITY_FILENAME, USER_STATE_DIR, normalize_rendezvous_url, normalize_with_scheme,
};
pub use doc::{
    DirectoryMeta, Doc, FileId, FileKind, FileMeta, Label, SCHEMA_VERSION, content_hash,
};
pub use error::{Error, Result};
pub use identity::{Identity, PUBKEY_LEN, Pubkey, SIGNATURE_LEN};
pub use net::{Frame, HelloOp};
pub use peers_md::{
    AuthorizedPeer, PEERS_FILE, parse_authorized_keys, parse_peers_md, render_authorized_keys,
    render_peers_md,
};

#[cfg(not(target_arch = "wasm32"))]
pub use fs::{BindOptions, Binding, NodeFsAdapter};
#[cfg(not(target_arch = "wasm32"))]
pub use identity::agent_list_identities_at;
#[cfg(not(target_arch = "wasm32"))]
pub use net::discover_vault_id;
#[cfg(not(target_arch = "wasm32"))]
pub use vault::{
    CreateOptions, CreatedVault, OpenOptions, ReconnectOptions, SyncHandle, Vault, VaultConfig,
    VaultEvent, VaultEventKind, VaultId,
};

/// Re-exports for the ssh-agent backend (Phase 3 of AUTH.md). Native-only.
#[cfg(not(target_arch = "wasm32"))]
pub mod agent {
    pub use crate::identity::agent_list_identities_at;
}
