use crate::error::Result;
use async_trait::async_trait;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct DirEntry {
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: u64,
}

#[async_trait]
pub trait FilesystemAdapter: Send + Sync {
    async fn read(&self, path: &Path) -> Result<Vec<u8>>;
    async fn write(&self, path: &Path, content: &[u8]) -> Result<()>;
    async fn delete(&self, path: &Path) -> Result<()>;
    async fn list(&self, path: &Path) -> Result<Vec<DirEntry>>;
    async fn exists(&self, path: &Path) -> bool;
    async fn hash(&self, path: &Path) -> Result<String>;
    fn watch(
        &self,
        path: &Path,
        sink: tokio::sync::mpsc::UnboundedSender<FsEvent>,
    ) -> Result<Box<dyn Watcher>>;
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
