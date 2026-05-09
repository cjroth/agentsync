//! Native storage adapters: tokio::fs-backed implementations of the
//! `DocStorage` / `BlobStorage` / `SnapshotStorage` traits.
//!
//! These are bytes-level adapters. The Doc serialization happens in the
//! Vault layer; this module is just "hand me bytes, I write bytes." The
//! existing `crate::store` types still hold the path layout knowledge —
//! these wrappers just expose them through the Host trait surface.

use crate::error::Result;
use crate::host::storage::{BlobStorage, DocStorage, SnapshotEntry, SnapshotStorage};
use crate::store::BlobStore;
use crate::store::snapshots::decode_b64_heads;
use async_trait::async_trait;
use automerge::ChangeHash;
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::fs;

pub struct NativeDocStorage {
    root: PathBuf,
}

impl NativeDocStorage {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
        }
    }

    fn doc_path(&self) -> PathBuf {
        self.root.join("doc.bin")
    }

    fn tmp_path(&self) -> PathBuf {
        self.root.join("doc.bin.tmp")
    }
}

#[async_trait(?Send)]
impl DocStorage for NativeDocStorage {
    async fn load(&self) -> Result<Option<Vec<u8>>> {
        match fs::read(self.doc_path()).await {
            Ok(b) => Ok(Some(b)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    async fn save(&self, bytes: &[u8]) -> Result<()> {
        let tmp = self.tmp_path();
        fs::write(&tmp, bytes).await?;
        fs::rename(&tmp, self.doc_path()).await?;
        Ok(())
    }

    async fn ensure_ready(&self) -> Result<()> {
        fs::create_dir_all(&self.root).await?;
        fs::create_dir_all(self.root.join("snapshots")).await?;
        fs::create_dir_all(self.root.join("blobs")).await?;
        Ok(())
    }
}

pub struct NativeBlobStorage {
    inner: BlobStore,
}

impl NativeBlobStorage {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            inner: BlobStore::new(root),
        }
    }
}

#[async_trait(?Send)]
impl BlobStorage for NativeBlobStorage {
    async fn has(&self, hash: &str) -> bool {
        self.inner.has(hash).await
    }

    async fn get(&self, hash: &str) -> Result<Vec<u8>> {
        self.inner.get(hash).await
    }

    async fn put(&self, bytes: &[u8]) -> Result<String> {
        self.inner.put(bytes).await
    }

    async fn put_with_hash(&self, hash: &str, bytes: &[u8]) -> Result<()> {
        self.inner.put_with_hash(hash, bytes).await
    }

    async fn ensure_ready(&self) -> Result<()> {
        self.inner.ensure_dirs().await
    }
}

/// On-disk JSON shape for the snapshot index. Mirrors the existing
/// `crate::store::snapshots::SnapshotIndexFile` layout — adapter writes the
/// same bytes the legacy `SnapshotIndex::write` would, so old vaults remain
/// readable and vice versa.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct OnDiskEntry {
    label: String,
    heads: String,
    created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct OnDiskFile {
    schema_version: i64,
    labels: Vec<OnDiskEntry>,
}

pub struct NativeSnapshotStorage {
    root: PathBuf,
}

impl NativeSnapshotStorage {
    pub fn new(storage_root: impl AsRef<Path>) -> Self {
        Self {
            root: storage_root.as_ref().join("snapshots"),
        }
    }

    fn index_path(&self) -> PathBuf {
        self.root.join("index.json")
    }
}

fn encode_heads(heads: &[ChangeHash]) -> String {
    let mut bytes = Vec::with_capacity(heads.len() * 32);
    for h in heads {
        bytes.extend_from_slice(h.as_ref());
    }
    base64::engine::general_purpose::STANDARD_NO_PAD.encode(bytes)
}

#[async_trait(?Send)]
impl SnapshotStorage for NativeSnapshotStorage {
    async fn read(&self) -> Result<Vec<SnapshotEntry>> {
        match fs::read(self.index_path()).await {
            Ok(bytes) => {
                let f: OnDiskFile = serde_json::from_slice(&bytes)?;
                Ok(f.labels
                    .into_iter()
                    .map(|e| SnapshotEntry {
                        label: e.label,
                        heads: decode_b64_heads(&e.heads).unwrap_or_default(),
                        created_at_ms: e.created_at,
                    })
                    .collect())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e.into()),
        }
    }

    async fn write(&self, entries: &[SnapshotEntry]) -> Result<()> {
        fs::create_dir_all(&self.root).await?;
        let on_disk = OnDiskFile {
            schema_version: 1,
            labels: entries
                .iter()
                .map(|e| OnDiskEntry {
                    label: e.label.clone(),
                    heads: encode_heads(&e.heads),
                    created_at: e.created_at_ms,
                })
                .collect(),
        };
        let json = serde_json::to_string_pretty(&on_disk)?;
        let tmp = self.root.join(".index.json.tmp");
        fs::write(&tmp, json.as_bytes()).await?;
        fs::rename(&tmp, self.index_path()).await?;
        Ok(())
    }

    async fn ensure_ready(&self) -> Result<()> {
        fs::create_dir_all(&self.root).await?;
        Ok(())
    }
}
