use crate::doc::content_hash;
use crate::error::{Error, Result};
use std::path::{Path, PathBuf};
use tokio::fs;

/// Content-addressed blob store at `<root>/blobs/<sha256>`.
pub struct BlobStore {
    root: PathBuf,
}

impl BlobStore {
    pub fn new(storage_root: impl AsRef<Path>) -> Self {
        Self {
            root: storage_root.as_ref().join("blobs"),
        }
    }

    fn blob_path(&self, hash: &str) -> PathBuf {
        self.root.join(hash)
    }

    pub async fn ensure_dirs(&self) -> Result<()> {
        fs::create_dir_all(&self.root).await?;
        Ok(())
    }

    pub async fn has(&self, hash: &str) -> bool {
        fs::metadata(self.blob_path(hash)).await.is_ok()
    }

    pub async fn get(&self, hash: &str) -> Result<Vec<u8>> {
        let bytes = fs::read(self.blob_path(hash))
            .await
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => Error::NotFound(format!("blob: {}", hash)),
                _ => Error::Io(e),
            })?;
        Ok(bytes)
    }

    /// Store bytes; returns the resulting hash.
    pub async fn put(&self, bytes: &[u8]) -> Result<String> {
        self.ensure_dirs().await?;
        let hash = content_hash(bytes);
        let path = self.blob_path(&hash);
        if !self.has(&hash).await {
            let tmp = self.root.join(format!(".tmp.{}", hash));
            fs::write(&tmp, bytes).await?;
            fs::rename(&tmp, &path).await?;
        }
        Ok(hash)
    }

    pub async fn put_with_hash(&self, hash: &str, bytes: &[u8]) -> Result<()> {
        self.ensure_dirs().await?;
        let computed = content_hash(bytes);
        if computed != hash {
            return Err(Error::Other(format!(
                "blob hash mismatch: expected {}, computed {}",
                hash, computed
            )));
        }
        let path = self.blob_path(hash);
        if !self.has(hash).await {
            let tmp = self.root.join(format!(".tmp.{}", hash));
            fs::write(&tmp, bytes).await?;
            fs::rename(&tmp, &path).await?;
        }
        Ok(())
    }
}
