use crate::doc::content_hash;
use crate::error::{Error, Result};
use crate::fs::adapter::{DirEntry, FilesystemAdapter, FsEvent, Watcher};
use async_trait::async_trait;
use notify::Watcher as NotifyWatcher;
use notify::{EventKind, RecommendedWatcher, RecursiveMode};
use std::path::Path;
use std::sync::Arc;
use tokio::fs;
use tokio::sync::mpsc;

pub struct NodeFsAdapter {}

impl NodeFsAdapter {
    pub fn new() -> Self {
        Self {}
    }
}

impl Default for NodeFsAdapter {
    fn default() -> Self {
        Self::new()
    }
}

struct NotifyHandle {
    _watcher: RecommendedWatcher,
}

impl Watcher for NotifyHandle {}

#[async_trait]
impl FilesystemAdapter for NodeFsAdapter {
    async fn read(&self, path: &Path) -> Result<Vec<u8>> {
        Ok(fs::read(path).await?)
    }

    async fn write(&self, path: &Path, content: &[u8]) -> Result<()> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).await?;
            }
        }
        let tmp = match path.file_name() {
            Some(name) => path.with_file_name(format!(".{}.agentsync-tmp", name.to_string_lossy())),
            None => return Err(Error::InvalidPath(path.display().to_string())),
        };
        fs::write(&tmp, content).await?;
        fs::rename(&tmp, path).await?;
        Ok(())
    }

    async fn delete(&self, path: &Path) -> Result<()> {
        match fs::remove_file(path).await {
            Ok(_) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    async fn list(&self, path: &Path) -> Result<Vec<DirEntry>> {
        let mut out = Vec::new();
        let mut rd = match fs::read_dir(path).await {
            Ok(r) => r,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(e.into()),
        };
        while let Some(entry) = rd.next_entry().await? {
            let md = entry.metadata().await?;
            out.push(DirEntry {
                path: entry.path(),
                is_dir: md.is_dir(),
                size: md.len(),
            });
        }
        Ok(out)
    }

    async fn exists(&self, path: &Path) -> bool {
        fs::metadata(path).await.is_ok()
    }

    async fn hash(&self, path: &Path) -> Result<String> {
        let bytes = fs::read(path).await?;
        Ok(content_hash(&bytes))
    }

    fn watch(&self, path: &Path, sink: mpsc::UnboundedSender<FsEvent>) -> Result<Box<dyn Watcher>> {
        let path = path.to_path_buf();
        let sink = Arc::new(sink);
        let sink_cloned = sink.clone();
        let mut watcher =
            notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                let event = match res {
                    Ok(e) => e,
                    Err(e) => {
                        tracing::warn!(error=%e, "notify error");
                        return;
                    }
                };
                for path in &event.paths {
                    let p = path.clone();
                    match event.kind {
                        EventKind::Create(_) | EventKind::Modify(_) => {
                            let _ = sink_cloned.send(FsEvent::Touched(p));
                        }
                        EventKind::Remove(_) => {
                            let _ = sink_cloned.send(FsEvent::Removed(p));
                        }
                        _ => {}
                    }
                }
            })?;
        watcher.watch(&path, RecursiveMode::Recursive)?;
        Ok(Box::new(NotifyHandle { _watcher: watcher }))
    }
}
