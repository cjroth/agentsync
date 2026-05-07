//! agentsync-core — real-time directory sync engine using Automerge CRDTs.
//!
//! See [`SPEC.md`] in the repo root for the full design. The core API lives on
//! [`Vault`].

pub mod auth;
pub mod constants;
pub mod doc;
pub mod error;
pub mod fs;
pub mod identity;
pub mod net;
pub mod path;
pub mod peers_md;
pub mod store;
pub mod tls;
pub mod vault;

pub use auth::{build_transcript, random_nonce, HANDSHAKE_DOMAIN, NONCE_LEN};
pub use constants::{
    normalize_rendezvous_url, AUTHORIZED_KEYS_FILE, DEFAULT_LISTEN_ADDR, DEFAULT_PORT,
    USER_IDENTITY_FILENAME, USER_STATE_DIR,
};
pub use doc::{
    content_hash, DirectoryMeta, Doc, FileId, FileKind, FileMeta, Label, SCHEMA_VERSION,
};
pub use error::{Error, Result};
pub use fs::{BindOptions, Binding, NodeFsAdapter};
pub use identity::{agent_list_identities_at, Identity, Pubkey, PUBKEY_LEN, SIGNATURE_LEN};

/// Re-exports for the ssh-agent backend (Phase 3 of AUTH.md).
pub mod agent {
    pub use crate::identity::agent_list_identities_at;
}
pub use net::{discover_vault_id, Frame, HelloOp};
pub use peers_md::{
    parse_authorized_keys, parse_peers_md, render_authorized_keys, render_peers_md,
    AuthorizedPeer, PEERS_FILE,
};
pub use vault::{
    CreateOptions, CreatedVault, OpenOptions, ReconnectOptions, SyncHandle, Vault, VaultConfig,
    VaultEvent, VaultEventKind, VaultId,
};
