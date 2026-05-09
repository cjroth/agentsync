//! Filesystem-side ingestion: convert disk state into Automerge changes.

use crate::constants::AUTHORIZED_KEYS_FILE;
use crate::doc::content_hash;
use crate::error::Result;
use crate::fs::adapter::FsEvent;
use crate::fs::binding::Binding;
use crate::fs::sync_ignore::SYNC_IGNORE_FILENAME;
use crate::vault::{VaultEvent, VaultEventKind, VaultInner};
use std::path::Path;
use std::sync::Arc;
use tracing::warn;
use walkdir::WalkDir;

fn is_syncignore(path: &Path) -> bool {
    path.file_name().and_then(|s| s.to_str()) == Some(SYNC_IGNORE_FILENAME)
}

pub(crate) async fn initial_scan(inner: &Arc<VaultInner>, binding: &Arc<Binding>) -> Result<()> {
    let root = binding.root().to_path_buf();
    let walker = WalkDir::new(&root).follow_links(false).into_iter();
    for entry in walker.filter_map(|e| e.ok()) {
        let abs = entry.path().to_path_buf();
        if entry.file_type().is_dir() {
            // The root itself isn't a directory we track inside the doc.
            if abs == root {
                continue;
            }
            if let Some(vault_path) = binding.fs_path_to_vault_dir_path(&abs) {
                ingest_directory(inner, binding, &vault_path).await?;
            }
        } else if entry.file_type().is_file() {
            if let Some(vault_path) = binding.fs_path_to_vault_path(&abs) {
                ingest_file(inner, binding, &vault_path).await?;
            }
        }
    }
    Ok(())
}

async fn ingest_directory(
    inner: &Arc<VaultInner>,
    binding: &Arc<Binding>,
    vault_path: &str,
) -> Result<()> {
    // Walk the entire subtree. notify's recursive watcher can miss events
    // for entries created inside a freshly-made subdirectory before the
    // watch on it is installed (the writes can land before the kernel adds
    // the inotify watch). Re-scanning the subtree closes that race for both
    // nested dirs and the files inside them.
    let abs_root = binding.vault_path_to_fs_path(vault_path);
    let mut changed = false;
    for entry in WalkDir::new(&abs_root).follow_links(false).into_iter().filter_map(|e| e.ok()) {
        let entry_abs = entry.path().to_path_buf();
        if entry.file_type().is_dir() {
            let p = match binding.fs_path_to_vault_dir_path(&entry_abs) {
                Some(p) => p,
                None => continue,
            };
            let mut doc = inner.doc.lock().await;
            let already = doc.find_directory_by_path(&p)?.is_some();
            if !already {
                doc.create_directory(&p)?;
                changed = true;
            }
            drop(doc);
            binding.materialized_dirs.lock().await.insert(p);
        } else if entry.file_type().is_file() {
            let p = match binding.fs_path_to_vault_path(&entry_abs) {
                Some(p) => p,
                None => continue,
            };
            ingest_file(inner, binding, &p).await?;
        }
    }
    if changed {
        inner.doc_changed.notify_waiters();
        let _ = inner.events.send(VaultEvent {
            kind: VaultEventKind::FileChanged {
                path: vault_path.to_string(),
            },
        });
    }
    Ok(())
}

pub(crate) async fn handle_fs_event(
    inner: &Arc<VaultInner>,
    binding: &Arc<Binding>,
    event: FsEvent,
) -> Result<()> {
    // Rebuild ignore matchers on any `.syncignore` change so mid-session edits
    // (or remote-pushed updates the materializer just wrote) take effect for
    // subsequent ingests in this same handler call. Done before the per-event
    // dispatch so the rebuilt patterns apply if the same event also targets
    // a file we'd otherwise consider for ingest.
    let touches_syncignore = match &event {
        FsEvent::Touched(p) | FsEvent::Removed(p) => is_syncignore(p),
        FsEvent::Renamed { from, to } => is_syncignore(from) || is_syncignore(to),
    };
    if touches_syncignore {
        binding.rebuild_sync_ignore();
    }
    let dispatch = dispatch_fs_event(inner, binding, event).await;
    if touches_syncignore {
        // A relaxed or removed rule may have un-excluded files that already
        // exist on disk. Walk the tree to ingest them — `ingest_file` is
        // idempotent for already-synced content, so this is a no-op when the
        // change only added rules.
        initial_scan(inner, binding).await?;
    }
    dispatch
}

async fn dispatch_fs_event(
    inner: &Arc<VaultInner>,
    binding: &Arc<Binding>,
    event: FsEvent,
) -> Result<()> {
    match event {
        FsEvent::Touched(abs) => {
            // Resolve disk type up-front: a single notify event might be for
            // a file or a directory (mkdir vs touch). We dispatch by kind,
            // not by path filter.
            let is_dir = match tokio::fs::metadata(&abs).await {
                Ok(md) => md.is_dir(),
                Err(_) => return Ok(()),
            };
            if is_dir {
                if let Some(vault_path) = binding.fs_path_to_vault_dir_path(&abs) {
                    ingest_directory(inner, binding, &vault_path).await?;
                }
            } else if let Some(vault_path) = binding.fs_path_to_vault_path(&abs) {
                ingest_file(inner, binding, &vault_path).await?;
            }
        }
        FsEvent::Removed(abs) => {
            // Disk is gone — but it may still appear if the event raced an
            // atomic rename. Re-check.
            if binding.adapter().exists(&abs).await {
                return Ok(());
            }
            // The path may correspond to either a file or a directory in
            // the doc. File deletes go through delete_file (single-path).
            // Directory deletes cascade so a recursive rm is captured even
            // when child events haven't been processed yet.
            if let Some(vault_path) = binding.fs_path_to_vault_path(&abs) {
                if vault_path == AUTHORIZED_KEYS_FILE {
                    // Refuse to propagate a deletion of authorized_keys.
                    // Without it the auth check returns an empty list and
                    // every peer is locked out of the vault — including the
                    // one that just did `rm -rf`. The materializer rewrites
                    // the file from the doc on its next pass.
                    return Ok(());
                }
                let mut doc = inner.doc.lock().await;
                if doc.file_exists(&vault_path) {
                    doc.delete_file(&vault_path)?;
                    drop(doc);
                    inner.doc_changed.notify_waiters();
                    let _ = inner.events.send(VaultEvent {
                        kind: VaultEventKind::FileChanged {
                            path: vault_path.clone(),
                        },
                    });
                    return Ok(());
                }
            }
            if let Some(vault_path) = binding.fs_path_to_vault_dir_path(&abs) {
                let mut doc = inner.doc.lock().await;
                if doc.find_directory_by_path(&vault_path)?.is_some() {
                    doc.delete_directory(&vault_path, true)?;
                    drop(doc);
                    binding
                        .materialized_dirs
                        .lock()
                        .await
                        .remove(&vault_path);
                    inner.doc_changed.notify_waiters();
                    let _ = inner.events.send(VaultEvent {
                        kind: VaultEventKind::FileChanged { path: vault_path },
                    });
                }
            }
        }
        FsEvent::Renamed { from, to } => {
            let from_v = binding.fs_path_to_vault_path(&from);
            let to_v = binding.fs_path_to_vault_path(&to);
            match (from_v, to_v) {
                (Some(f), Some(t)) => {
                    let mut doc = inner.doc.lock().await;
                    if doc.file_exists(&f) && !doc.file_exists(&t) {
                        doc.rename_file(&f, &t)?;
                        drop(doc);
                        inner.doc_changed.notify_waiters();
                    }
                }
                (Some(f), None) => {
                    if f == AUTHORIZED_KEYS_FILE {
                        // Same protection as FsEvent::Removed: a rename out
                        // of the vault is a delete from the doc's view.
                        return Ok(());
                    }
                    let mut doc = inner.doc.lock().await;
                    if doc.file_exists(&f) {
                        doc.delete_file(&f)?;
                        drop(doc);
                        inner.doc_changed.notify_waiters();
                    }
                }
                (None, Some(t)) => {
                    ingest_file(inner, binding, &t).await?;
                }
                (None, None) => {}
            }
        }
    }
    Ok(())
}

async fn ingest_file(
    inner: &Arc<VaultInner>,
    binding: &Arc<Binding>,
    vault_path: &str,
) -> Result<()> {
    let abs = binding.vault_path_to_fs_path(vault_path);
    let bytes = match binding.adapter().read(&abs).await {
        Ok(b) => b,
        Err(_) => return Ok(()),
    };

    // Guard against the slow-truncate-write pattern: some editors leave the
    // file empty for >150ms between O_TRUNC and the actual write, which is
    // longer than our event debounce window. Ingesting that transient empty
    // state would propagate as a real change and the materializer on peers
    // would write empty over their disks. If we see empty on disk but the
    // doc still has non-empty content, wait 300ms and re-read; in the common
    // case the editor has finished by then. Genuine emptying just gets a
    // small extra latency.
    let bytes = if bytes.is_empty() {
        let doc_has_content = {
            let mut doc = inner.doc.lock().await;
            doc.read_file(vault_path)
                .map(|s| !s.is_empty())
                .unwrap_or(false)
        };
        if doc_has_content {
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            match binding.adapter().read(&abs).await {
                Ok(b) => b,
                Err(_) => return Ok(()),
            }
        } else {
            bytes
        }
    } else {
        bytes
    };

    let size = bytes.len() as u64;
    if binding.over_size(vault_path, size) {
        warn!(vault_path, size, "skipping oversize file");
        return Ok(());
    }
    let hash = content_hash(&bytes);

    // Loop suppression check: if this hash matches a recent core write, ignore.
    {
        let mut dirty = binding.dirty.lock().await;
        if dirty.check_and_consume(vault_path, &hash) {
            return Ok(());
        }
    }

    if binding.is_text_extension(vault_path) {
        let s = match std::str::from_utf8(&bytes) {
            Ok(s) => s.to_string(),
            Err(_) => {
                // Fall back to attachment path if not valid utf-8.
                let h = inner.blob_store.put(&bytes).await?;
                let mut doc = inner.doc.lock().await;
                doc.write_attachment(vault_path, &h, size as i64)?;
                drop(doc);
                inner.doc_changed.notify_waiters();
                let _ = inner.events.send(VaultEvent {
                    kind: VaultEventKind::FileChanged {
                        path: vault_path.to_string(),
                    },
                });
                binding
                    .materialized
                    .lock()
                    .await
                    .insert(vault_path.to_string(), h.clone());
                binding
                    .last_ingested
                    .lock()
                    .await
                    .insert(vault_path.to_string(), h);
                return Ok(());
            }
        };
        // Idempotency: skip if doc already has this exact content.
        {
            let mut doc = inner.doc.lock().await;
            if let Ok(cur) = doc.read_file(vault_path) {
                if cur == s {
                    binding
                        .materialized
                        .lock()
                        .await
                        .insert(vault_path.to_string(), hash.clone());
                    binding
                        .last_ingested
                        .lock()
                        .await
                        .insert(vault_path.to_string(), hash);
                    return Ok(());
                }
            }
        }
        let mut doc = inner.doc.lock().await;
        doc.write_text_file(vault_path, &s)?;
        drop(doc);
        inner.doc_changed.notify_waiters();
        let _ = inner.events.send(VaultEvent {
            kind: VaultEventKind::FileChanged {
                path: vault_path.to_string(),
            },
        });
        binding
            .materialized
            .lock()
            .await
            .insert(vault_path.to_string(), hash.clone());
        binding
            .last_ingested
            .lock()
            .await
            .insert(vault_path.to_string(), hash);
    } else {
        let h = inner.blob_store.put(&bytes).await?;
        let mut doc = inner.doc.lock().await;
        doc.write_attachment(vault_path, &h, size as i64)?;
        drop(doc);
        inner.doc_changed.notify_waiters();
        let _ = inner.events.send(VaultEvent {
            kind: VaultEventKind::FileChanged {
                path: vault_path.to_string(),
            },
        });
        binding
            .materialized
            .lock()
            .await
            .insert(vault_path.to_string(), h.clone());
        binding
            .last_ingested
            .lock()
            .await
            .insert(vault_path.to_string(), h);
    }
    Ok(())
}
