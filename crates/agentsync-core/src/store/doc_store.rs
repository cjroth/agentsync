use crate::doc::Doc;
use crate::error::{Error, Result};
use std::path::{Path, PathBuf};
use tokio::fs;

/// Persists the Automerge document to `<root>/doc.bin` with atomic-rename writes.
pub struct DocStore {
    root: PathBuf,
}

impl DocStore {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
        }
    }

    pub fn doc_path(&self) -> PathBuf {
        self.root.join("doc.bin")
    }

    pub fn tmp_path(&self) -> PathBuf {
        self.root.join("doc.bin.tmp")
    }

    pub async fn ensure_dirs(&self) -> Result<()> {
        fs::create_dir_all(&self.root).await?;
        fs::create_dir_all(self.root.join("snapshots")).await?;
        fs::create_dir_all(self.root.join("blobs")).await?;
        Ok(())
    }

    pub async fn doc_exists(&self) -> bool {
        fs::metadata(self.doc_path()).await.is_ok()
    }

    pub async fn load(&self) -> Result<Doc> {
        let bytes = fs::read(self.doc_path()).await?;
        Doc::load(&bytes)
    }

    pub async fn save(&self, doc: &mut Doc) -> Result<()> {
        let bytes = doc.save();
        let tmp = self.tmp_path();
        fs::write(&tmp, &bytes).await?;
        // Atomic rename (best-effort across filesystems).
        fs::rename(&tmp, self.doc_path()).await.map_err(|e| {
            Error::Io(std::io::Error::new(
                e.kind(),
                format!("rename doc.bin failed: {}", e),
            ))
        })?;
        Ok(())
    }
}
