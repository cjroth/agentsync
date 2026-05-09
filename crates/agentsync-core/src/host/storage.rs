//! Storage abstractions for the three on-disk artifacts a vault keeps:
//! `doc.bin` (Automerge save), `blobs/<hash>` (CAS attachments), and
//! `snapshots.json` (label index). Native impl uses `tokio::fs`; wasm impl
//! delegates to a JS-supplied object that backs onto OPFS, IndexedDB, or
//! `node:fs` depending on the runtime.
//!
//! Implementations MUST be atomic at the granularity of one full `save` /
//! `put` call — torn writes are unrecoverable. The native impl achieves
//! this via write-to-tmp + rename; OPFS uses `FileSystemSyncAccessHandle`'s
//! atomic `flush`. IndexedDB transactions are atomic by construction.

use crate::error::Result;
use async_trait::async_trait;
use automerge::ChangeHash;

/// Persists the full Automerge document. Tnly one `doc.bin` exists per
/// vault; the storage adapter is responsible for atomic replacement.
#[async_trait(?Send)]
pub trait DocStorage: Send + Sync + 'static {
    /// Load saved bytes. `Ok(None)` when no doc has ever been saved.
    async fn load(&self) -> Result<Option<Vec<u8>>>;
    /// Replace doc.bin atomically.
    async fn save(&self, bytes: &[u8]) -> Result<()>;
    /// Create any directories the implementation needs. Idempotent.
    async fn ensure_ready(&self) -> Result<()>;
}

/// Content-addressed blob store. Used for binary attachments (anything
/// outside the small-file inline budget).
#[async_trait(?Send)]
pub trait BlobStorage: Send + Sync + 'static {
    async fn has(&self, hash: &str) -> bool;
    /// Returns the stored bytes for `hash`. `Err` if not present.
    async fn get(&self, hash: &str) -> Result<Vec<u8>>;
    /// Hash and store; returns the hex-encoded SHA-256.
    async fn put(&self, bytes: &[u8]) -> Result<String>;
    /// Store under a caller-supplied hash (used when the hash is already
    /// known from a network frame). Implementations should verify the hash
    /// matches before persisting.
    async fn put_with_hash(&self, hash: &str, bytes: &[u8]) -> Result<()>;
    async fn ensure_ready(&self) -> Result<()>;
}

/// Snapshot / label index. One JSON-ish file per vault holding the named
/// historical heads pointers.
#[async_trait(?Send)]
pub trait SnapshotStorage: Send + Sync + 'static {
    async fn read(&self) -> Result<Vec<SnapshotEntry>>;
    async fn write(&self, entries: &[SnapshotEntry]) -> Result<()>;
    async fn ensure_ready(&self) -> Result<()>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotEntry {
    pub label: String,
    pub heads: Vec<ChangeHash>,
    pub created_at_ms: i64,
}
