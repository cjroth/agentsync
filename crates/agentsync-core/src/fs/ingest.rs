//! Filesystem-side ingestion: convert disk state into Automerge changes.

use crate::doc::content_hash;
use crate::error::Result;
use crate::fs::adapter::FsEvent;
use crate::fs::binding::Binding;
use crate::vault::{VaultEvent, VaultEventKind, VaultInner};
use std::sync::Arc;
use tracing::warn;
use walkdir::WalkDir;

pub(crate) async fn initial_scan(inner: &Arc<VaultInner>, binding: &Arc<Binding>) -> Result<()> {
    let root = binding.root().to_path_buf();
    let walker = WalkDir::new(&root).follow_links(false).into_iter();
    for entry in walker.filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        let abs = entry.path().to_path_buf();
        let vault_path = match binding.fs_path_to_vault_path(&abs) {
            Some(p) => p,
            None => continue,
        };
        ingest_file(inner, binding, &vault_path).await?;
    }
    Ok(())
}

pub(crate) async fn handle_fs_event(
    inner: &Arc<VaultInner>,
    binding: &Arc<Binding>,
    event: FsEvent,
) -> Result<()> {
    match event {
        FsEvent::Touched(abs) => {
            if let Some(vault_path) = binding.fs_path_to_vault_path(&abs) {
                ingest_file(inner, binding, &vault_path).await?;
            }
        }
        FsEvent::Removed(abs) => {
            if let Some(vault_path) = binding.fs_path_to_vault_path(&abs) {
                let exists_on_disk = binding.adapter().exists(&abs).await;
                if exists_on_disk {
                    return Ok(());
                }
                let mut doc = inner.doc.lock().await;
                if doc.file_exists(&vault_path) {
                    doc.delete_file(&vault_path)?;
                    drop(doc);
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
