//! Native filesystem adapter wrapping the existing `NodeFsAdapter` with the
//! two new methods (`create_dir_all`, `remove_dir`) the host trait requires.
//! The original `crate::fs::adapter::FilesystemAdapter` stays in place for
//! the materializer's internal calls during the trait-extraction phase; this
//! adapter is what `Host::filesystem()` returns.

use crate::error::Result;
use crate::fs::adapter::{
    DirEntry as InnerDirEntry, FilesystemAdapter as InnerFilesystemAdapter,
    FsEvent as InnerFsEvent, Watcher as InnerWatcher,
};
use crate::fs::node_adapter::NodeFsAdapter;
use crate::host::filesystem::{
    DirEntry as HostDirEntry, FilesystemAdapter, FsEvent as HostFsEvent, Watcher as HostWatcher,
};
use async_trait::async_trait;
use std::path::Path;
use tokio::fs;
use tokio::sync::mpsc::UnboundedSender;

pub struct NativeFilesystem {
    inner: NodeFsAdapter,
}

impl NativeFilesystem {
    pub fn new() -> Self {
        Self {
            inner: NodeFsAdapter::new(),
        }
    }
}

impl Default for NativeFilesystem {
    fn default() -> Self {
        Self::new()
    }
}

struct WatcherShim {
    _inner: Box<dyn InnerWatcher>,
}

impl HostWatcher for WatcherShim {}

fn map_dir_entry(e: InnerDirEntry) -> HostDirEntry {
    HostDirEntry {
        path: e.path,
        is_dir: e.is_dir,
        size: e.size,
    }
}

#[async_trait(?Send)]
impl FilesystemAdapter for NativeFilesystem {
    async fn read(&self, path: &Path) -> Result<Vec<u8>> {
        self.inner.read(path).await
    }

    async fn write(&self, path: &Path, content: &[u8]) -> Result<()> {
        self.inner.write(path, content).await
    }

    async fn delete(&self, path: &Path) -> Result<()> {
        self.inner.delete(path).await
    }

    async fn list(&self, path: &Path) -> Result<Vec<HostDirEntry>> {
        Ok(self
            .inner
            .list(path)
            .await?
            .into_iter()
            .map(map_dir_entry)
            .collect())
    }

    async fn exists(&self, path: &Path) -> bool {
        self.inner.exists(path).await
    }

    async fn hash(&self, path: &Path) -> Result<String> {
        self.inner.hash(path).await
    }

    async fn create_dir_all(&self, path: &Path) -> Result<()> {
        Ok(fs::create_dir_all(path).await?)
    }

    async fn remove_dir(&self, path: &Path) -> Result<()> {
        match fs::remove_dir(path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    fn watch(
        &self,
        path: &Path,
        sink: UnboundedSender<HostFsEvent>,
    ) -> Result<Box<dyn HostWatcher>> {
        // Bridge: the inner adapter takes its own FsEvent type; spawn a
        // forwarder mapping inner events to host events.
        let (inner_tx, mut inner_rx) = tokio::sync::mpsc::unbounded_channel::<InnerFsEvent>();
        let host_sink = sink.clone();
        tokio::spawn(async move {
            while let Some(event) = inner_rx.recv().await {
                let mapped = match event {
                    InnerFsEvent::Touched(p) => HostFsEvent::Touched(p),
                    InnerFsEvent::Removed(p) => HostFsEvent::Removed(p),
                    InnerFsEvent::Renamed { from, to } => HostFsEvent::Renamed { from, to },
                };
                if host_sink.send(mapped).is_err() {
                    break;
                }
            }
        });
        let inner = self.inner.watch(path, inner_tx)?;
        Ok(Box::new(WatcherShim { _inner: inner }))
    }
}
