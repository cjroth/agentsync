//! agentsync-core — real-time directory sync engine using Automerge CRDTs.
//!
//! See [`SPEC.md`] in the repo root for the full design. The core API lives on
//! [`Vault`].

pub mod auth;
pub mod doc;
pub mod error;
pub mod fs;
pub mod net;
pub mod path;
pub mod store;
pub mod vault;

pub use auth::{decode_key, encode_key, generate_vault_key, VaultKey, VAULT_KEY_LEN};
pub use doc::{
    content_hash, DirectoryMeta, Doc, FileId, FileKind, FileMeta, Label, SCHEMA_VERSION,
};
pub use error::{Error, Result};
pub use fs::{BindOptions, Binding, NodeFsAdapter};
pub use net::{discover_vault_id, Frame, HelloOp};
pub use vault::{
    CreateOptions, CreatedVault, OpenOptions, ReconnectOptions, SyncHandle, Vault, VaultConfig,
    VaultEvent, VaultEventKind, VaultId,
};
