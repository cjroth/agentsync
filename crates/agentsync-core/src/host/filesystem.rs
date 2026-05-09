//! Filesystem adapter for the *bound directory* — the user-visible folder
//! that the Vault is mirroring. This is distinct from the storage layer
//! (`.agentsync/` internals); the bound directory is where real files live.
//!
//! The native impl is `notify` + `tokio::fs`. The wasm impl is supplied by
//! JS and may be `node:fs` (Electron, Tauri, VS Code) or the File System
//! Access API (browser apps). Browsers without a backing folder run in
//! storage-only mode where this trait is `None` on the Host bundle.
//!
//! This trait was previously `crate::fs::adapter::FilesystemAdapter`; it's
//! moved here as part of the host abstraction layer. The two new methods
//! (`create_dir_all`, `remove_dir`) replace the only non-abstracted FS
//! callsites in the materializer.

use crate::error::Result;
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use tokio::sync::mpsc::UnboundedSender;

#[derive(Debug, Clone)]
pub struct DirEntry {
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: u64,
}

#[async_trait(?Send)]
pub trait FilesystemAdapter: Send + Sync + 'static {
    async fn read(&self, path: &Path) -> Result<Vec<u8>>;
    async fn write(&self, path: &Path, content: &[u8]) -> Result<()>;
    async fn delete(&self, path: &Path) -> Result<()>;
    async fn list(&self, path: &Path) -> Result<Vec<DirEntry>>;
    async fn exists(&self, path: &Path) -> bool;
    async fn hash(&self, path: &Path) -> Result<String>;
    /// Recursively create a directory and any missing parents. Idempotent.
    async fn create_dir_all(&self, path: &Path) -> Result<()>;
    /// Remove an empty directory. Errors if the directory is non-empty.
    async fn remove_dir(&self, path: &Path) -> Result<()>;
    /// Install a watcher for `path`. Events flow into `sink`. Drop the
    /// returned [`Watcher`] to stop watching. Wasm impls running in pure
    /// storage mode return an error here.
    fn watch(&self, path: &Path, sink: UnboundedSender<FsEvent>) -> Result<Box<dyn Watcher>>;
}

pub trait Watcher: Send + Sync {}

#[derive(Debug, Clone)]
pub enum FsEvent {
    /// A file was created or modified at this absolute path.
    Touched(PathBuf),
    /// A file or directory was removed at this absolute path.
    Removed(PathBuf),
    /// A rename: old path → new path.
    Renamed { from: PathBuf, to: PathBuf },
}
