use crate::auth::{decode_key, encode_key, generate_vault_key, VaultKey, VAULT_KEY_LEN};
use crate::doc::{content_hash, Doc, FileKind, FileMeta, Label};
use crate::error::{Error, Result};
use crate::fs::adapter::{FilesystemAdapter, FsEvent};
use crate::fs::binding::{BindOptions, Binding};
use crate::fs::node_adapter::NodeFsAdapter;
use crate::net::client::ClientConn;
use crate::net::protocol::Frame;
use crate::net::server::Server;
use crate::path as path_norm;
use crate::store::{BlobStore, DocStore, SnapshotIndex};
use async_trait::async_trait;
use automerge::sync::{self as amsync, SyncDoc};
use automerge::ChangeHash;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, Mutex, Notify};
use tokio::time::{interval, Duration};
use tracing::{debug, info, warn};
use uuid::Uuid;

pub type VaultId = String;

#[derive(Debug, Clone)]
pub struct OpenOptions {
    pub rendezvous_url: Option<String>,
    pub vault_id: VaultId,
    pub vault_key: VaultKey,
    pub storage_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct CreateOptions {
    pub rendezvous_url: Option<String>,
    pub vault_key: Option<VaultKey>,
    pub storage_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct VaultEvent {
    pub kind: VaultEventKind,
}

#[derive(Debug, Clone)]
pub enum VaultEventKind {
    Connected,
    Disconnected,
    FileChanged { path: String },
    SyncProgress { percent: u8 },
    Error(String),
}

#[derive(Debug, Clone)]
pub struct CreatedVault {
    pub vault_id: VaultId,
    pub vault_key: VaultKey,
}

/// Owns one Automerge doc, the storage layout, and any active network sessions.
pub struct Vault {
    inner: Arc<VaultInner>,
}

pub(crate) struct VaultInner {
    pub vault_id: VaultId,
    pub vault_key: VaultKey,
    pub storage_path: PathBuf,
    pub doc: Mutex<Doc>,
    pub doc_store: DocStore,
    pub blob_store: BlobStore,
    pub snapshots: SnapshotIndex,
    pub binding: Mutex<Option<Arc<Binding>>>,

    // sync hub
    pub peers: Mutex<HashMap<u64, PeerSlot>>,
    pub next_peer_id: AtomicU64,
    pub doc_changed: Notify,

    // events
    pub events: broadcast::Sender<VaultEvent>,

    // active outbound connection (if any)
    pub client: Mutex<Option<ClientConn>>,
    pub server: Mutex<Option<Server>>,

    pub config: VaultConfig,
}

#[derive(Debug, Clone)]
pub struct VaultConfig {
    pub rendezvous_url: Option<String>,
    pub save_interval: Duration,
    pub save_after_changes: u32,
}

impl Default for VaultConfig {
    fn default() -> Self {
        Self {
            rendezvous_url: None,
            save_interval: Duration::from_secs(1),
            save_after_changes: 100,
        }
    }
}

pub struct PeerSlot {
    pub state: amsync::State,
    pub out: mpsc::UnboundedSender<Frame>,
}

impl Vault {
    /// Create a brand-new vault on disk. Generates vault_id and key if absent.
    pub async fn create(opts: CreateOptions) -> Result<(Self, CreatedVault)> {
        let storage = opts.storage_path.clone();
        tokio::fs::create_dir_all(&storage).await?;
        let doc_store = DocStore::new(&storage);
        let blob_store = BlobStore::new(&storage);
        let snapshots = SnapshotIndex::new(&storage);
        doc_store.ensure_dirs().await?;
        blob_store.ensure_dirs().await?;

        let vault_id = Uuid::new_v4().to_string();
        let vault_key = opts.vault_key.unwrap_or_else(generate_vault_key);

        if doc_store.doc_exists().await {
            return Err(Error::AlreadyExists(format!(
                "doc.bin already present at {}",
                doc_store.doc_path().display()
            )));
        }
        let mut doc = Doc::new(&vault_id)?;
        doc_store.save(&mut doc).await?;

        let inner = Arc::new(VaultInner {
            vault_id: vault_id.clone(),
            vault_key,
            storage_path: storage,
            doc: Mutex::new(doc),
            doc_store,
            blob_store,
            snapshots,
            binding: Mutex::new(None),
            peers: Mutex::new(HashMap::new()),
            next_peer_id: AtomicU64::new(1),
            doc_changed: Notify::new(),
            events: broadcast::channel(64).0,
            client: Mutex::new(None),
            server: Mutex::new(None),
            config: VaultConfig {
                rendezvous_url: opts.rendezvous_url,
                ..Default::default()
            },
        });
        let v = Vault {
            inner: inner.clone(),
        };
        v.start_save_loop();
        Ok((
            v,
            CreatedVault {
                vault_id,
                vault_key,
            },
        ))
    }

    pub async fn open(opts: OpenOptions) -> Result<Self> {
        let storage = opts.storage_path.clone();
        let doc_store = DocStore::new(&storage);
        let blob_store = BlobStore::new(&storage);
        let snapshots = SnapshotIndex::new(&storage);
        doc_store.ensure_dirs().await?;
        blob_store.ensure_dirs().await?;
        let mut doc = if doc_store.doc_exists().await {
            doc_store.load().await?
        } else {
            Doc::new(&opts.vault_id)?
        };
        let stored_id = doc.vault_id().unwrap_or_else(|_| opts.vault_id.clone());
        if stored_id != opts.vault_id {
            return Err(Error::Vault(format!(
                "doc.bin vault_id mismatch: doc has {}, opts has {}",
                stored_id, opts.vault_id
            )));
        }
        let inner = Arc::new(VaultInner {
            vault_id: opts.vault_id,
            vault_key: opts.vault_key,
            storage_path: storage,
            doc: Mutex::new(doc),
            doc_store,
            blob_store,
            snapshots,
            binding: Mutex::new(None),
            peers: Mutex::new(HashMap::new()),
            next_peer_id: AtomicU64::new(1),
            doc_changed: Notify::new(),
            events: broadcast::channel(64).0,
            client: Mutex::new(None),
            server: Mutex::new(None),
            config: VaultConfig {
                rendezvous_url: opts.rendezvous_url,
                ..Default::default()
            },
        });
        let v = Vault {
            inner: inner.clone(),
        };
        v.start_save_loop();
        Ok(v)
    }

    pub fn id(&self) -> &VaultId {
        &self.inner.vault_id
    }

    pub fn key(&self) -> &VaultKey {
        &self.inner.vault_key
    }

    pub fn key_b64(&self) -> String {
        encode_key(&self.inner.vault_key)
    }

    pub fn storage_path(&self) -> &Path {
        &self.inner.storage_path
    }

    pub fn subscribe(&self) -> broadcast::Receiver<VaultEvent> {
        self.inner.events.subscribe()
    }

    /// Save the doc to disk now.
    pub async fn flush(&self) -> Result<()> {
        let mut doc = self.inner.doc.lock().await;
        self.inner.doc_store.save(&mut doc).await
    }

    /// Background loop: periodically flush the doc.
    fn start_save_loop(&self) {
        let inner = self.inner.clone();
        let interval_dur = inner.config.save_interval;
        tokio::spawn(async move {
            let mut tick = interval(interval_dur);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tick.tick().await;
                let mut doc = inner.doc.lock().await;
                if let Err(e) = inner.doc_store.save(&mut doc).await {
                    warn!(error=%e, "doc save failed");
                }
            }
        });
    }

    // ---------- file ops ----------

    pub async fn write_text_file(&self, path: &str, content: &str) -> Result<()> {
        {
            let mut doc = self.inner.doc.lock().await;
            doc.write_text_file(path, content)?;
        }
        self.notify_doc_changed();
        let _ = self.inner.events.send(VaultEvent {
            kind: VaultEventKind::FileChanged {
                path: path.to_string(),
            },
        });
        Ok(())
    }

    pub async fn read_text_file(&self, path: &str) -> Result<String> {
        let mut doc = self.inner.doc.lock().await;
        doc.read_file(path)
    }

    pub async fn delete_file(&self, path: &str) -> Result<()> {
        {
            let mut doc = self.inner.doc.lock().await;
            doc.delete_file(path)?;
        }
        self.notify_doc_changed();
        let _ = self.inner.events.send(VaultEvent {
            kind: VaultEventKind::FileChanged {
                path: path.to_string(),
            },
        });
        Ok(())
    }

    pub async fn rename_file(&self, from: &str, to: &str) -> Result<()> {
        {
            let mut doc = self.inner.doc.lock().await;
            doc.rename_file(from, to)?;
        }
        self.notify_doc_changed();
        Ok(())
    }

    pub async fn list_files(&self) -> Result<Vec<FileMeta>> {
        let mut doc = self.inner.doc.lock().await;
        doc.list_files()
    }

    pub async fn list_file_paths(&self) -> Result<Vec<String>> {
        let mut doc = self.inner.doc.lock().await;
        doc.list_file_paths()
    }

    pub async fn file_exists(&self, path: &str) -> bool {
        let mut doc = self.inner.doc.lock().await;
        doc.file_exists(path)
    }

    pub async fn file_hash(&self, path: &str) -> Result<String> {
        let mut doc = self.inner.doc.lock().await;
        doc.file_hash(path)
    }

    pub async fn create_directory(&self, path: &str) -> Result<()> {
        {
            let mut doc = self.inner.doc.lock().await;
            doc.create_directory(path)?;
        }
        self.notify_doc_changed();
        Ok(())
    }

    pub async fn delete_directory(&self, path: &str, recursive: bool) -> Result<()> {
        {
            let mut doc = self.inner.doc.lock().await;
            doc.delete_directory(path, recursive)?;
        }
        self.notify_doc_changed();
        Ok(())
    }

    pub async fn rename_directory(&self, from: &str, to: &str) -> Result<()> {
        {
            let mut doc = self.inner.doc.lock().await;
            doc.rename_directory(from, to)?;
        }
        self.notify_doc_changed();
        Ok(())
    }

    // ---------- history ops ----------

    pub async fn create_label(&self, name: &str) -> Result<()> {
        {
            let mut doc = self.inner.doc.lock().await;
            doc.create_label(name)?;
        }
        self.flush_snapshots_index().await?;
        self.notify_doc_changed();
        Ok(())
    }

    pub async fn delete_label(&self, name: &str) -> Result<()> {
        {
            let mut doc = self.inner.doc.lock().await;
            doc.delete_label(name)?;
        }
        self.flush_snapshots_index().await?;
        self.notify_doc_changed();
        Ok(())
    }

    pub async fn list_labels(&self) -> Result<Vec<Label>> {
        let mut doc = self.inner.doc.lock().await;
        doc.list_labels()
    }

    pub async fn restore_label(&self, name: &str) -> Result<()> {
        let heads = {
            let mut doc = self.inner.doc.lock().await;
            doc.get_label_heads(name)?
        };
        self.restore_to_heads(&heads).await?;
        // Materialize.
        if let Some(b) = self.binding_arc().await {
            self.materialize(&b).await?;
        }
        Ok(())
    }

    pub async fn restore_to_heads(&self, heads: &[ChangeHash]) -> Result<()> {
        {
            let mut doc = self.inner.doc.lock().await;
            doc.restore_to_heads(heads)?;
        }
        self.notify_doc_changed();
        Ok(())
    }

    pub async fn restore_to_time(&self, target_ms: i64) -> Result<()> {
        {
            let mut doc = self.inner.doc.lock().await;
            doc.restore_to_time(target_ms)?;
        }
        self.notify_doc_changed();
        Ok(())
    }

    async fn flush_snapshots_index(&self) -> Result<()> {
        let labels = {
            let mut doc = self.inner.doc.lock().await;
            doc.list_labels()?
        };
        self.inner.snapshots.write(&labels).await
    }

    // ---------- networking ----------

    /// Connect to the configured rendezvous (if any).
    pub async fn connect(&mut self) -> Result<()> {
        let url = match self.inner.config.rendezvous_url.clone() {
            Some(u) => u,
            None => return Err(Error::Network("no rendezvous_url configured".into())),
        };
        let conn = ClientConn::connect(
            &url,
            self.inner.vault_id.clone(),
            self.inner.vault_key,
            Arc::new(VaultSyncHandle {
                inner: self.inner.clone(),
            }) as Arc<dyn SyncHandle>,
        )
        .await?;
        *self.inner.client.lock().await = Some(conn);
        let _ = self.inner.events.send(VaultEvent {
            kind: VaultEventKind::Connected,
        });
        Ok(())
    }

    /// Drop the active outbound connection (if any).
    pub async fn disconnect(&mut self) {
        *self.inner.client.lock().await = None;
        let _ = self.inner.events.send(VaultEvent {
            kind: VaultEventKind::Disconnected,
        });
    }

    /// Bind the active server (`agentsync --listen`) on `addr`.
    pub async fn listen(&mut self, addr: SocketAddr) -> Result<SocketAddr> {
        let server = Server::bind(
            addr,
            self.inner.vault_id.clone(),
            self.inner.vault_key,
            Arc::new(VaultSyncHandle {
                inner: self.inner.clone(),
            }) as Arc<dyn SyncHandle>,
        )
        .await?;
        let bound = server.bound_addr;
        *self.inner.server.lock().await = Some(server);
        Ok(bound)
    }

    pub fn notify_doc_changed(&self) {
        self.inner.doc_changed.notify_waiters();
    }

    // ---------- binding ----------

    pub async fn bind_directory(
        &mut self,
        path: &Path,
        opts: BindOptions,
    ) -> Result<Arc<Binding>> {
        // Initial scan: ingest existing files into the doc.
        let adapter: Arc<dyn FilesystemAdapter> = Arc::new(NodeFsAdapter::new());
        let mut binding = Binding::new(path, opts.clone(), adapter.clone());

        // Set up watcher channel.
        let (tx, mut rx) = mpsc::unbounded_channel::<FsEvent>();
        let watcher = adapter.watch(path, tx)?;
        binding.set_watcher(watcher);
        let binding = Arc::new(binding);
        *self.inner.binding.lock().await = Some(binding.clone());

        // Initial scan & ingest.
        crate::fs::ingest::initial_scan(&self.inner, &binding).await?;

        // Initial materialization for any files already in the doc that are
        // missing from disk (e.g. clone case).
        self.materialize(&binding).await?;

        // Inbound watcher loop.
        {
            let inner = self.inner.clone();
            let binding = binding.clone();
            tokio::spawn(async move {
                while let Some(ev) = rx.recv().await {
                    if let Err(e) =
                        crate::fs::ingest::handle_fs_event(&inner, &binding, ev).await
                    {
                        warn!(error=%e, "fs event handler");
                    }
                }
            });
        }

        // Doc-change → materialize loop. Uses both a Notify and a short
        // periodic poll, so that a notification dropped while the task is
        // not parked still gets caught on the next tick.
        {
            let inner = self.inner.clone();
            let binding = binding.clone();
            tokio::spawn(async move {
                loop {
                    if let Err(e) = materialize_inner(&inner, &binding).await {
                        warn!(error=%e, "materialize");
                    }
                    tokio::select! {
                        _ = inner.doc_changed.notified() => {}
                        _ = tokio::time::sleep(Duration::from_millis(100)) => {}
                    }
                }
            });
        }
        Ok(binding)
    }

    pub async fn binding_arc(&self) -> Option<Arc<Binding>> {
        self.inner.binding.lock().await.clone()
    }

    pub async fn materialize(&self, binding: &Arc<Binding>) -> Result<()> {
        materialize_inner(&self.inner, binding).await
    }

    /// Save and gracefully shut down. Drop alone is also fine.
    pub async fn close(self) -> Result<()> {
        self.flush().await?;
        Ok(())
    }
}

/// Bridge between the network layer and the Vault. Implemented by `VaultSyncHandle`.
#[async_trait]
pub trait SyncHandle: Send + Sync {
    async fn register_peer(&self, out: mpsc::UnboundedSender<Frame>) -> Result<u64>;
    async fn unregister_peer(&self, peer_id: u64);
    async fn generate_sync_message(&self, peer_id: u64) -> Result<Option<Vec<u8>>>;
    async fn receive_sync_message(&self, peer_id: u64, bytes: &[u8]) -> Result<()>;
    async fn read_blob(&self, hash: &str) -> Result<Vec<u8>>;
    async fn write_blob(&self, hash: &str, bytes: &[u8]) -> Result<()>;
    async fn wait_doc_changed(&self);
}

pub(crate) struct VaultSyncHandle {
    pub inner: Arc<VaultInner>,
}

#[async_trait]
impl SyncHandle for VaultSyncHandle {
    async fn register_peer(&self, out: mpsc::UnboundedSender<Frame>) -> Result<u64> {
        let id = self.inner.next_peer_id.fetch_add(1, Ordering::SeqCst);
        self.inner.peers.lock().await.insert(
            id,
            PeerSlot {
                state: amsync::State::new(),
                out,
            },
        );
        debug!(peer_id = id, "peer registered");
        Ok(id)
    }
    async fn unregister_peer(&self, peer_id: u64) {
        self.inner.peers.lock().await.remove(&peer_id);
    }
    async fn generate_sync_message(&self, peer_id: u64) -> Result<Option<Vec<u8>>> {
        let mut peers = self.inner.peers.lock().await;
        let slot = match peers.get_mut(&peer_id) {
            Some(s) => s,
            None => return Ok(None),
        };
        let mut doc = self.inner.doc.lock().await;
        let mut sd = doc.inner.sync();
        let msg = sd.generate_sync_message(&mut slot.state);
        let bytes = msg.map(|m| m.encode());
        tracing::debug!(peer_id, has = bytes.is_some(), "generated sync message");
        Ok(bytes)
    }
    async fn receive_sync_message(&self, peer_id: u64, bytes: &[u8]) -> Result<()> {
        tracing::debug!(peer_id, n = bytes.len(), "receiving sync message");
        let msg = amsync::Message::decode(bytes)
            .map_err(|e| Error::Protocol(format!("sync decode: {}", e)))?;
        let mut peers = self.inner.peers.lock().await;
        let slot = match peers.get_mut(&peer_id) {
            Some(s) => s,
            None => return Err(Error::Protocol(format!("unknown peer {}", peer_id))),
        };
        let before;
        let after;
        {
            let mut doc = self.inner.doc.lock().await;
            before = doc.inner.get_heads();
            {
                let mut sd = doc.inner.sync();
                sd.receive_sync_message(&mut slot.state, msg)
                    .map_err(|e| Error::Other(format!("receive sync: {}", e)))?;
            }
            after = doc.inner.get_heads();
        }
        drop(peers);
        if before != after {
            self.inner.doc_changed.notify_waiters();
        }
        Ok(())
    }
    async fn read_blob(&self, hash: &str) -> Result<Vec<u8>> {
        self.inner.blob_store.get(hash).await
    }
    async fn write_blob(&self, hash: &str, bytes: &[u8]) -> Result<()> {
        self.inner.blob_store.put_with_hash(hash, bytes).await
    }
    async fn wait_doc_changed(&self) {
        self.inner.doc_changed.notified().await;
    }
}

// -- materialization (doc -> disk) --

async fn materialize_inner(inner: &Arc<VaultInner>, binding: &Arc<Binding>) -> Result<()> {
    let files = {
        let mut doc = inner.doc.lock().await;
        doc.list_files()?
    };
    tracing::trace!(count = files.len(), "materialize: scanning live files");
    let live: HashMap<String, FileMeta> =
        files.into_iter().map(|m| (m.path.clone(), m)).collect();

    // Snapshot of what we currently believe is on disk.
    let existing: HashMap<String, String> = binding.materialized.lock().await.clone();

    // Removals: anything we previously wrote but is no longer in `live`.
    for path in existing.keys() {
        if !live.contains_key(path) {
            let abs = binding.vault_path_to_fs_path(path);
            if let Err(e) = binding.adapter().delete(&abs).await {
                warn!(path, error=%e, "delete from disk");
            } else {
                let mut dirty = binding.dirty.lock().await;
                dirty.mark(path, "<deleted>");
            }
        }
    }

    // Writes/updates.
    for (path, meta) in &live {
        match meta.kind {
            FileKind::Text => {
                let target = {
                    let mut doc = inner.doc.lock().await;
                    doc.read_file_id(&meta.id)?.unwrap_or_default()
                };
                let target_hash = content_hash(target.as_bytes());
                if existing.get(path).map(|h| h.as_str()) == Some(target_hash.as_str()) {
                    continue;
                }
                let abs = binding.vault_path_to_fs_path(path);
                {
                    let mut dirty = binding.dirty.lock().await;
                    dirty.mark(path, &target_hash);
                }
                if let Err(e) = binding.adapter().write(&abs, target.as_bytes()).await {
                    warn!(path, error=%e, "write to disk");
                    continue;
                }
                binding
                    .materialized
                    .lock()
                    .await
                    .insert(path.clone(), target_hash);
            }
            FileKind::Attachment => {
                if let Some(h) = &meta.binary_hash {
                    let bytes = match inner.blob_store.get(h).await {
                        Ok(b) => b,
                        Err(_) => continue, // blob not yet available
                    };
                    let cur = existing.get(path).cloned();
                    if cur.as_deref() == Some(h.as_str()) {
                        continue;
                    }
                    let abs = binding.vault_path_to_fs_path(path);
                    {
                        let mut dirty = binding.dirty.lock().await;
                        dirty.mark(path, h);
                    }
                    if let Err(e) = binding.adapter().write(&abs, &bytes).await {
                        warn!(path, error=%e, "write attachment");
                        continue;
                    }
                    binding
                        .materialized
                        .lock()
                        .await
                        .insert(path.clone(), h.clone());
                }
            }
        }
    }

    // Drop entries from `materialized` that we removed.
    let mut mat = binding.materialized.lock().await;
    mat.retain(|p, _| live.contains_key(p));
    Ok(())
}

/// Helper for tests: peek live file paths.
impl Vault {
    pub async fn debug_dump(&self) -> Result<Vec<(String, Option<String>)>> {
        let mut doc = self.inner.doc.lock().await;
        let mut out = Vec::new();
        for f in doc.list_files()? {
            let content = match f.kind {
                FileKind::Text => doc.read_file(&f.path).ok(),
                FileKind::Attachment => f.binary_hash.clone(),
            };
            out.push((f.path, content));
        }
        Ok(out)
    }
}
