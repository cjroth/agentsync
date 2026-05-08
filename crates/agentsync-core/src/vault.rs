use crate::doc::{content_hash, Doc, FileKind, FileMeta, Label};
use crate::error::{Error, Result};
use crate::fs::adapter::{FilesystemAdapter, FsEvent};
use crate::fs::binding::{BindOptions, Binding};
use crate::fs::node_adapter::NodeFsAdapter;
use crate::identity::{Identity, Pubkey};
use crate::net::client::ClientConn;
use crate::net::protocol::Frame;
use crate::net::server::Server;
use crate::constants::AUTHORIZED_KEYS_FILE;
use crate::peers_md::{parse_authorized_keys, render_authorized_keys, AuthorizedPeer};
use crate::store::{BlobStore, DocStore, SnapshotIndex};
use async_trait::async_trait;
use automerge::sync::{self as amsync, SyncDoc};
use automerge::ChangeHash;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, oneshot, Mutex, Notify};
use tokio::task::JoinHandle;
use tokio::time::{interval, Duration};
use tracing::{debug, info, warn};
use uuid::Uuid;

pub type VaultId = String;

#[derive(Debug, Clone)]
pub struct OpenOptions {
    pub rendezvous_url: Option<String>,
    pub vault_id: VaultId,
    pub identity: Identity,
    pub storage_path: PathBuf,
    /// TOFU-pinned hub identity (Phase 4 of AUTH.md). When `Some`, every
    /// outbound connect requires the hub's pubkey to match exactly. When
    /// `None`, any hub identity is accepted; the caller is responsible for
    /// running the trust prompt if interactive.
    pub hub_pubkey: Option<Pubkey>,
    /// Display name for this vault, sent in the handshake so cloning peers
    /// can default the local directory name.
    pub name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CreateOptions {
    pub rendezvous_url: Option<String>,
    /// Optional pre-generated identity. When `None`, a fresh ed25519 keypair
    /// is generated.
    pub identity: Option<Identity>,
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
    pub identity: Identity,
}

/// Owns one Automerge doc, the storage layout, and any active network sessions.
pub struct Vault {
    inner: Arc<VaultInner>,
}

pub(crate) struct VaultInner {
    pub vault_id: VaultId,
    pub identity: Identity,
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
    pub(crate) reconnect_supervisor: Mutex<Option<ReconnectSupervisor>>,

    pub config: VaultConfig,
}

#[derive(Debug, Clone)]
pub struct ReconnectOptions {
    pub max_attempts: u32,
    pub initial_backoff: Duration,
    pub max_backoff: Duration,
}

impl Default for ReconnectOptions {
    fn default() -> Self {
        Self {
            max_attempts: 10,
            initial_backoff: Duration::from_millis(500),
            max_backoff: Duration::from_secs(30),
        }
    }
}

pub(crate) struct ReconnectSupervisor {
    pub shutdown_tx: oneshot::Sender<()>,
    pub handle: JoinHandle<()>,
}

impl ReconnectSupervisor {
    async fn shutdown(self) {
        let _ = self.shutdown_tx.send(());
        let _ = tokio::time::timeout(Duration::from_secs(2), self.handle).await;
    }
}

fn backoff_delay(initial: Duration, cap: Duration, attempt: u32) -> Duration {
    let exp = attempt.saturating_sub(1).min(20);
    let factor = 1u64 << exp;
    let millis = (initial.as_millis() as u64).saturating_mul(factor);
    Duration::from_millis(millis).min(cap)
}

enum ConnectResult {
    Connected(ClientConn),
    Shutdown,
    GaveUp,
}

async fn connect_with_backoff(
    inner: &Arc<VaultInner>,
    url: &str,
    opts: &ReconnectOptions,
    shutdown_rx: &mut oneshot::Receiver<()>,
) -> ConnectResult {
    let mut attempt: u32 = 0;
    loop {
        attempt += 1;
        let result = ClientConn::connect(
            url,
            Some(inner.vault_id.clone()),
            inner.config.hub_pubkey,
            inner.identity.clone(),
            Arc::new(VaultSyncHandle {
                inner: inner.clone(),
            }) as Arc<dyn SyncHandle>,
        )
        .await;
        match result {
            Ok(c) => {
                info!(url, attempt, "connected to rendezvous");
                return ConnectResult::Connected(c);
            }
            Err(e) => {
                if attempt >= opts.max_attempts {
                    warn!(
                        url,
                        attempt,
                        error = %e,
                        "connect attempt failed (max retries reached)"
                    );
                    let _ = inner.events.send(VaultEvent {
                        kind: VaultEventKind::Error(format!(
                            "could not connect to rendezvous after {} attempts: {}",
                            attempt, e
                        )),
                    });
                    return ConnectResult::GaveUp;
                }
                let delay = backoff_delay(opts.initial_backoff, opts.max_backoff, attempt);
                warn!(
                    url,
                    attempt,
                    error = %e,
                    retry_in_ms = delay.as_millis() as u64,
                    "connect attempt failed; retrying"
                );
                tokio::select! {
                    _ = tokio::time::sleep(delay) => {}
                    _ = &mut *shutdown_rx => return ConnectResult::Shutdown,
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct VaultConfig {
    pub rendezvous_url: Option<String>,
    pub hub_pubkey: Option<Pubkey>,
    pub name: Option<String>,
    pub save_interval: Duration,
    pub save_after_changes: u32,
}

impl Default for VaultConfig {
    fn default() -> Self {
        Self {
            rendezvous_url: None,
            hub_pubkey: None,
            name: None,
            save_interval: Duration::from_secs(1),
            save_after_changes: 100,
        }
    }
}

pub struct PeerSlot {
    pub state: amsync::State,
    pub out: mpsc::UnboundedSender<Frame>,
    /// Set during the handshake; used by the server's authorization enforcer
    /// to find peers no longer in `authorized_keys`.
    pub pubkey: Option<Pubkey>,
}

impl Vault {
    /// Create a brand-new vault on disk. Generates vault_id and identity if absent.
    /// Also seeds `authorized_keys` with the creator's pubkey so they can connect to
    /// their own listener immediately.
    pub async fn create(opts: CreateOptions) -> Result<(Self, CreatedVault)> {
        let storage = opts.storage_path.clone();
        tokio::fs::create_dir_all(&storage).await?;
        let doc_store = DocStore::new(&storage);
        let blob_store = BlobStore::new(&storage);
        let snapshots = SnapshotIndex::new(&storage);
        doc_store.ensure_dirs().await?;
        blob_store.ensure_dirs().await?;

        let vault_id = Uuid::new_v4().to_string();
        let identity = opts.identity.unwrap_or_else(Identity::generate);

        if doc_store.doc_exists().await {
            return Err(Error::AlreadyExists(format!(
                "doc.bin already present at {}",
                doc_store.doc_path().display()
            )));
        }
        let mut doc = Doc::new(&vault_id)?;
        // Seed authorized_keys so the creator's own pubkey is authorized — otherwise
        // every later connection (including their own listener accepting their
        // own client) would be rejected.
        let seed = render_authorized_keys(&[AuthorizedPeer {
            pubkey: identity.pubkey(),
            label: "creator".into(),
        }]);
        doc.write_text_file(AUTHORIZED_KEYS_FILE, &seed)?;
        doc_store.save(&mut doc).await?;

        let inner = Arc::new(VaultInner {
            vault_id: vault_id.clone(),
            identity: identity.clone(),
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
            reconnect_supervisor: Mutex::new(None),
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
                identity,
            },
        ))
    }

    /// The display name carried in the handshake (from `OpenOptions.name`).
    pub fn name(&self) -> Option<&str> {
        self.inner.config.name.as_deref()
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
            identity: opts.identity,
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
            reconnect_supervisor: Mutex::new(None),
            config: VaultConfig {
                rendezvous_url: opts.rendezvous_url,
                hub_pubkey: opts.hub_pubkey,
                name: opts.name,
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

    pub fn identity(&self) -> &Identity {
        &self.inner.identity
    }

    pub fn pubkey(&self) -> Pubkey {
        self.inner.identity.pubkey()
    }

    pub fn storage_path(&self) -> &Path {
        &self.inner.storage_path
    }

    pub fn subscribe(&self) -> broadcast::Receiver<VaultEvent> {
        self.inner.events.subscribe()
    }

    pub async fn flush(&self) -> Result<()> {
        let mut doc = self.inner.doc.lock().await;
        self.inner.doc_store.save(&mut doc).await
    }

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

    pub async fn list_directories(&self) -> Result<Vec<crate::doc::DirectoryMeta>> {
        let mut doc = self.inner.doc.lock().await;
        doc.list_directories()
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
            Some(self.inner.vault_id.clone()),
            self.inner.config.hub_pubkey,
            self.inner.identity.clone(),
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

    pub async fn disconnect(&mut self) {
        let supervisor = self.inner.reconnect_supervisor.lock().await.take();
        if let Some(s) = supervisor {
            s.shutdown().await;
        }
        let conn = self.inner.client.lock().await.take();
        if let Some(c) = conn {
            c.close().await;
        }
        let _ = self.inner.events.send(VaultEvent {
            kind: VaultEventKind::Disconnected,
        });
    }

    pub async fn connect_with_reconnect(&mut self, opts: ReconnectOptions) -> Result<()> {
        self.disconnect().await;

        let url = match self.inner.config.rendezvous_url.clone() {
            Some(u) => u,
            None => return Err(Error::Network("no rendezvous_url configured".into())),
        };

        let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();
        let inner = self.inner.clone();
        let handle = tokio::spawn(async move {
            'outer: loop {
                let conn = match connect_with_backoff(
                    &inner,
                    &url,
                    &opts,
                    &mut shutdown_rx,
                )
                .await
                {
                    ConnectResult::Connected(c) => c,
                    ConnectResult::Shutdown => return,
                    ConnectResult::GaveUp => {
                        warn!(
                            url = %url,
                            attempts = opts.max_attempts,
                            "could not reach rendezvous; giving up"
                        );
                        return;
                    }
                };

                let _ = inner.events.send(VaultEvent {
                    kind: VaultEventKind::Connected,
                });

                let closed = conn.closed_signal();
                {
                    let mut slot = inner.client.lock().await;
                    *slot = Some(conn);
                }
                tokio::select! {
                    _ = closed.wait() => {
                        warn!(url = %url, "lost connection to rendezvous; will retry");
                        let _ = inner.events.send(VaultEvent {
                            kind: VaultEventKind::Disconnected,
                        });
                        let dead = inner.client.lock().await.take();
                        if let Some(c) = dead {
                            c.close().await;
                        }
                        continue 'outer;
                    }
                    _ = &mut shutdown_rx => {
                        return;
                    }
                }
            }
        });

        *self.inner.reconnect_supervisor.lock().await = Some(ReconnectSupervisor {
            shutdown_tx,
            handle,
        });
        Ok(())
    }

    /// Bind a listener with a self-signed TLS cert. The cert is auto-loaded
    /// or generated under `<storage>/../.agentsync-server/`.
    pub async fn listen(&mut self, addr: SocketAddr) -> Result<SocketAddr> {
        let tls_dir = crate::tls::tls_dir_for_storage(&self.inner.storage_path);
        let (cert_der, key_der) = crate::tls::load_or_generate_self_signed(&tls_dir)?;
        let server = Server::bind(
            addr,
            self.inner.vault_id.clone(),
            self.inner.config.name.clone(),
            self.inner.identity.clone(),
            Arc::new(VaultSyncHandle {
                inner: self.inner.clone(),
            }) as Arc<dyn SyncHandle>,
            cert_der,
            key_der,
        )
        .await?;
        let bound = server.bound_addr;
        *self.inner.server.lock().await = Some(server);
        Ok(bound)
    }

    pub async fn unlisten(&mut self) {
        let server = self.inner.server.lock().await.take();
        if let Some(s) = server {
            s.shutdown().await;
        }
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
        let adapter: Arc<dyn FilesystemAdapter> = Arc::new(NodeFsAdapter::new());
        let mut binding = Binding::new(path, opts.clone(), adapter.clone());

        let (tx, rx) = mpsc::unbounded_channel::<FsEvent>();
        let watcher = adapter.watch(path, tx)?;
        binding.set_watcher(watcher);
        let binding = Arc::new(binding);
        *self.inner.binding.lock().await = Some(binding.clone());

        crate::fs::ingest::initial_scan(&self.inner, &binding).await?;

        self.materialize(&binding).await?;

        {
            let inner = self.inner.clone();
            let binding = binding.clone();
            tokio::spawn(async move {
                debounced_fs_loop(inner, binding, rx).await;
            });
        }

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

    pub async fn close(mut self) -> Result<()> {
        self.disconnect().await;
        self.unlisten().await;
        self.flush().await?;
        Ok(())
    }

    pub async fn peer_count(&self) -> usize {
        self.inner.peers.lock().await.len()
    }

    /// Read & parse `authorized_keys` from the synced doc. Empty list if the file is
    /// missing or unparseable.
    pub async fn authorized_pubkeys(&self) -> Vec<Pubkey> {
        let mut doc = self.inner.doc.lock().await;
        let content = match doc.read_file(AUTHORIZED_KEYS_FILE) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        parse_authorized_keys(&content)
            .into_iter()
            .map(|p| p.pubkey)
            .collect()
    }
}

#[async_trait]
pub trait SyncHandle: Send + Sync {
    async fn register_peer(
        &self,
        out: mpsc::UnboundedSender<Frame>,
        pubkey: Option<Pubkey>,
    ) -> Result<u64>;
    async fn unregister_peer(&self, peer_id: u64);
    async fn generate_sync_message(&self, peer_id: u64) -> Result<Option<Vec<u8>>>;
    async fn receive_sync_message(&self, peer_id: u64, bytes: &[u8]) -> Result<()>;
    async fn read_blob(&self, hash: &str) -> Result<Vec<u8>>;
    async fn write_blob(&self, hash: &str, bytes: &[u8]) -> Result<()>;
    async fn wait_doc_changed(&self);
    /// Latest list of authorized peer pubkeys, as parsed from `authorized_keys` in
    /// the synced doc.
    async fn authorized_pubkeys(&self) -> Vec<Pubkey>;
    /// Latest authorized peers including labels. Used by the enforcer to
    /// log human-readable peer-add/remove notices. Default impl adapts
    /// [`Self::authorized_pubkeys`] with empty labels.
    async fn authorized_peers(&self) -> Vec<AuthorizedPeer> {
        self.authorized_pubkeys()
            .await
            .into_iter()
            .map(|pk| AuthorizedPeer {
                pubkey: pk,
                label: String::new(),
            })
            .collect()
    }
    /// Drop the outbound channel for any connected peer whose pubkey is not
    /// in `authorized`. The peer's writer task observes the channel close and
    /// shuts down the websocket gracefully.
    async fn disconnect_unauthorized_peers(&self, authorized: &[Pubkey]);
}

pub(crate) struct VaultSyncHandle {
    pub inner: Arc<VaultInner>,
}

#[async_trait]
impl SyncHandle for VaultSyncHandle {
    async fn register_peer(
        &self,
        out: mpsc::UnboundedSender<Frame>,
        pubkey: Option<Pubkey>,
    ) -> Result<u64> {
        let id = self.inner.next_peer_id.fetch_add(1, Ordering::SeqCst);
        self.inner.peers.lock().await.insert(
            id,
            PeerSlot {
                state: amsync::State::new(),
                out,
                pubkey,
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
        let sd = doc.inner.sync();
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
            // Late sync message for a peer we just disconnected (e.g. it
            // was deauthorized via authorized_keys). Silently drop — the
            // disconnect was logged separately, and treating this as an
            // error produces confusing "unknown peer N" warnings.
            None => {
                tracing::debug!(peer_id, "ignoring sync message for disconnected peer");
                return Ok(());
            }
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
    async fn authorized_pubkeys(&self) -> Vec<Pubkey> {
        let mut doc = self.inner.doc.lock().await;
        let content = match doc.read_file(AUTHORIZED_KEYS_FILE) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        parse_authorized_keys(&content)
            .into_iter()
            .map(|p| p.pubkey)
            .collect()
    }
    async fn authorized_peers(&self) -> Vec<AuthorizedPeer> {
        let mut doc = self.inner.doc.lock().await;
        let content = match doc.read_file(AUTHORIZED_KEYS_FILE) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        parse_authorized_keys(&content)
    }
    async fn disconnect_unauthorized_peers(&self, authorized: &[Pubkey]) {
        let mut peers = self.inner.peers.lock().await;
        let to_drop: Vec<(u64, Option<Pubkey>)> = peers
            .iter()
            .filter_map(|(id, slot)| match slot.pubkey {
                Some(pk) if !authorized.contains(&pk) => Some((*id, Some(pk))),
                _ => None,
            })
            .collect();
        for (id, pk) in to_drop {
            if let Some(slot) = peers.remove(&id) {
                drop(slot);
                match pk {
                    Some(pk) => info!(
                        peer_id = id,
                        fp = %pk.fingerprint_sha256(),
                        "disconnecting peer (no longer in authorized_keys)",
                    ),
                    None => info!(peer_id = id, "disconnecting unidentified peer"),
                }
            }
        }
    }
}

const FS_DEBOUNCE: Duration = Duration::from_millis(150);

async fn debounced_fs_loop(
    inner: Arc<VaultInner>,
    binding: Arc<Binding>,
    mut rx: mpsc::UnboundedReceiver<FsEvent>,
) {
    use std::path::PathBuf;
    use tokio::time::Instant;

    let mut pending: HashMap<PathBuf, (FsEvent, Instant)> = HashMap::new();

    loop {
        let now = Instant::now();
        let next_wait = pending
            .values()
            .map(|(_, t)| t.saturating_duration_since(now))
            .min()
            .unwrap_or(Duration::from_secs(3600));

        tokio::select! {
            biased;
            ev = rx.recv() => {
                match ev {
                    Some(ev) => {
                        let path = match &ev {
                            FsEvent::Touched(p) | FsEvent::Removed(p) => p.clone(),
                            FsEvent::Renamed { to, .. } => to.clone(),
                        };
                        pending.insert(path, (ev, Instant::now() + FS_DEBOUNCE));
                    }
                    None => {
                        flush_expired(&inner, &binding, &mut pending, true).await;
                        return;
                    }
                }
            }
            _ = tokio::time::sleep(next_wait) => {
                flush_expired(&inner, &binding, &mut pending, false).await;
            }
        }
    }
}

async fn flush_expired(
    inner: &Arc<VaultInner>,
    binding: &Arc<Binding>,
    pending: &mut HashMap<std::path::PathBuf, (FsEvent, tokio::time::Instant)>,
    force_all: bool,
) {
    let now = tokio::time::Instant::now();
    let expired: Vec<std::path::PathBuf> = pending
        .iter()
        .filter(|(_, (_, t))| force_all || *t <= now)
        .map(|(p, _)| p.clone())
        .collect();
    for path in expired {
        if let Some((ev, _)) = pending.remove(&path) {
            if let Err(e) = crate::fs::ingest::handle_fs_event(inner, binding, ev).await {
                warn!(error=%e, "fs event handler");
            }
        }
    }
}

async fn materialize_inner(inner: &Arc<VaultInner>, binding: &Arc<Binding>) -> Result<()> {
    let (files, dirs) = {
        let mut doc = inner.doc.lock().await;
        (doc.list_files()?, doc.list_directories()?)
    };
    tracing::trace!(count = files.len(), "materialize: scanning live files");
    let live: HashMap<String, FileMeta> =
        files.into_iter().map(|m| (m.path.clone(), m)).collect();
    let live_dirs: std::collections::HashSet<String> =
        dirs.into_iter().map(|d| d.path).collect();

    {
        let mut materialized_dirs = binding.materialized_dirs.lock().await;
        let mut ordered: Vec<&String> = live_dirs
            .iter()
            .filter(|p| !materialized_dirs.contains(*p))
            .collect();
        ordered.sort_by_key(|p| (p.matches('/').count(), p.as_str().to_string()));
        for path in ordered {
            let abs = binding.vault_path_to_fs_path(path);
            if let Err(e) = tokio::fs::create_dir_all(&abs).await {
                warn!(path, error=%e, "create directory");
                continue;
            }
            materialized_dirs.insert(path.clone());
        }
    }

    let existing: HashMap<String, String> = binding.materialized.lock().await.clone();
    let last_ingested: HashMap<String, String> =
        binding.last_ingested.lock().await.clone();

    for path in existing.keys() {
        if !live.contains_key(path) {
            let abs = binding.vault_path_to_fs_path(path);
            let disk_hash = match binding.adapter().read(&abs).await {
                Ok(b) => Some(content_hash(&b)),
                Err(_) => None,
            };
            if let Some(d) = &disk_hash {
                if !is_safe_to_overwrite(d, existing.get(path), last_ingested.get(path)) {
                    tracing::debug!(
                        path,
                        "materialize: deferring delete — disk has uncommitted local edits"
                    );
                    continue;
                }
            }
            if let Err(e) = binding.adapter().delete(&abs).await {
                warn!(path, error=%e, "delete from disk");
            } else {
                let mut dirty = binding.dirty.lock().await;
                dirty.mark(path, "<deleted>");
            }
        }
    }

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

                let disk_hash = match binding.adapter().read(&abs).await {
                    Ok(b) => Some(content_hash(&b)),
                    Err(_) => None,
                };
                if let Some(d) = &disk_hash {
                    if d.as_str() == target_hash.as_str() {
                        binding
                            .materialized
                            .lock()
                            .await
                            .insert(path.clone(), target_hash);
                        continue;
                    }
                    if !is_safe_to_overwrite(d, existing.get(path), last_ingested.get(path)) {
                        tracing::debug!(
                            path,
                            "materialize: deferring write — disk has uncommitted local edits"
                        );
                        continue;
                    }
                }

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
                        Err(_) => continue,
                    };
                    let cur = existing.get(path).cloned();
                    if cur.as_deref() == Some(h.as_str()) {
                        continue;
                    }
                    let abs = binding.vault_path_to_fs_path(path);
                    let disk_hash = match binding.adapter().read(&abs).await {
                        Ok(b) => Some(content_hash(&b)),
                        Err(_) => None,
                    };
                    if let Some(d) = &disk_hash {
                        if d.as_str() == h.as_str() {
                            binding
                                .materialized
                                .lock()
                                .await
                                .insert(path.clone(), h.clone());
                            continue;
                        }
                        if !is_safe_to_overwrite(d, existing.get(path), last_ingested.get(path)) {
                            tracing::debug!(
                                path,
                                "materialize: deferring attachment write — disk has uncommitted local edits"
                            );
                            continue;
                        }
                    }
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

    binding
        .materialized
        .lock()
        .await
        .retain(|p, _| live.contains_key(p));
    binding
        .last_ingested
        .lock()
        .await
        .retain(|p, _| live.contains_key(p));

    {
        let mut materialized_dirs = binding.materialized_dirs.lock().await;
        let to_remove: Vec<String> = materialized_dirs
            .iter()
            .filter(|p| !live_dirs.contains(*p))
            .cloned()
            .collect();
        let mut ordered = to_remove.clone();
        ordered.sort_by_key(|p| std::cmp::Reverse(p.matches('/').count()));
        for path in ordered {
            let abs = binding.vault_path_to_fs_path(&path);
            match tokio::fs::remove_dir(&abs).await {
                Ok(_) => {
                    materialized_dirs.remove(&path);
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    materialized_dirs.remove(&path);
                }
                Err(e) => {
                    tracing::debug!(path, error=%e, "materialize: skipping non-empty dir");
                }
            }
        }
    }
    Ok(())
}

fn is_safe_to_overwrite(
    disk_hash: &str,
    last_materialized: Option<&String>,
    last_ingested: Option<&String>,
) -> bool {
    last_materialized.map(|s| s.as_str()) == Some(disk_hash)
        || last_ingested.map(|s| s.as_str()) == Some(disk_hash)
}

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
