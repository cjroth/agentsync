//! Host abstraction layer.
//!
//! The `Vault` is generic over its environment by holding `Arc<dyn Host>`.
//! Every OS-touching operation — spawning tasks, reading the clock, opening
//! sockets, persisting bytes, watching filesystems, generating randomness,
//! signing — flows through this trait. Native hosts are tokio + rustls +
//! notify + disk; wasm hosts are JS-supplied shims.
//!
//! Sub-trait getter methods return `&dyn ...` (not `&'static dyn ...`) so
//! impls can keep state per-host. The optional methods (`listener`,
//! `filesystem`, `tls`) return `None` on hosts that genuinely cannot
//! support that capability — browsers can't bind listeners or terminate
//! TLS themselves, and pure-CRDT browser apps may run without a
//! filesystem at all.

pub mod crypto;
pub mod filesystem;
pub mod runtime;
pub mod storage;
pub mod transport;

pub mod native;

pub use crypto::{Rng, Signer, TlsCert, TlsCertProvider};
pub use filesystem::{DirEntry, FilesystemAdapter, FsEvent, Watcher};
pub use runtime::{Clock, SpawnHandle, SpawnHandleImpl, Spawner};
pub use storage::{BlobStorage, DocStorage, SnapshotEntry, SnapshotStorage};
pub use transport::{Acceptor, Conn, ConnectOpts, Listener, TlsConfig, Transport};

/// The environment a Vault runs in. Native and wasm hosts both implement
/// this; tests mock individual sub-traits and use a [`HostBuilder`] to
/// assemble custom bundles.
pub trait Host: Send + Sync + 'static {
    fn spawner(&self) -> &dyn Spawner;
    fn clock(&self) -> &dyn Clock;
    fn rng(&self) -> &dyn Rng;
    fn transport(&self) -> &dyn Transport;
    /// Inbound listener. Browsers cannot listen; Node could (currently
    /// unused on wasm). Native always has one.
    fn listener(&self) -> Option<&dyn Listener>;
    fn doc_storage(&self) -> &dyn DocStorage;
    fn blob_storage(&self) -> &dyn BlobStorage;
    fn snapshot_storage(&self) -> &dyn SnapshotStorage;
    /// Bound-directory adapter. `None` for storage-only mode (browser apps
    /// without a backing directory).
    fn filesystem(&self) -> Option<&dyn FilesystemAdapter>;
    /// Native-only. Wasm transport handles its own TLS via the underlying
    /// JS WebSocket implementation.
    fn tls(&self) -> Option<&dyn TlsCertProvider>;
}
