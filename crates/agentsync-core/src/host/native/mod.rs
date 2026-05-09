//! Native (tokio + rustls + notify + disk) implementations of the host
//! traits, plus a `native_host` builder that assembles them all into a
//! single `Arc<dyn Host>`.

pub mod crypto;
pub mod filesystem;
pub mod runtime;
pub mod storage;
pub mod transport;

use crate::host::{
    BlobStorage, Clock, DocStorage, FilesystemAdapter, Host, Listener, Rng, SnapshotStorage,
    Spawner, TlsCertProvider, Transport,
};
use std::path::PathBuf;
use std::sync::Arc;

use crypto::{NativeTlsProvider, OsRngProvider};
use filesystem::NativeFilesystem;
use runtime::{SystemClock, TokioSpawner};
use storage::{NativeBlobStorage, NativeDocStorage, NativeSnapshotStorage};
use transport::{NativeListener, NativeTransport};

/// Bundles every native trait impl into a single `Host`. Constructed by
/// [`native_host`].
pub struct NativeHost {
    spawner: TokioSpawner,
    clock: SystemClock,
    rng: OsRngProvider,
    transport: NativeTransport,
    listener: NativeListener,
    doc_storage: NativeDocStorage,
    blob_storage: NativeBlobStorage,
    snapshot_storage: NativeSnapshotStorage,
    filesystem: NativeFilesystem,
    tls: NativeTlsProvider,
}

impl Host for NativeHost {
    fn spawner(&self) -> &dyn Spawner {
        &self.spawner
    }
    fn clock(&self) -> &dyn Clock {
        &self.clock
    }
    fn rng(&self) -> &dyn Rng {
        &self.rng
    }
    fn transport(&self) -> &dyn Transport {
        &self.transport
    }
    fn listener(&self) -> Option<&dyn Listener> {
        Some(&self.listener)
    }
    fn doc_storage(&self) -> &dyn DocStorage {
        &self.doc_storage
    }
    fn blob_storage(&self) -> &dyn BlobStorage {
        &self.blob_storage
    }
    fn snapshot_storage(&self) -> &dyn SnapshotStorage {
        &self.snapshot_storage
    }
    fn filesystem(&self) -> Option<&dyn FilesystemAdapter> {
        Some(&self.filesystem)
    }
    fn tls(&self) -> Option<&dyn TlsCertProvider> {
        Some(&self.tls)
    }
}

/// Build a `NativeHost` rooted at `storage_path` (the `.agentsync/` dir for
/// this vault). Storage adapters share the root; transport / listener /
/// filesystem are runtime singletons.
pub fn native_host(storage_path: PathBuf) -> Arc<dyn Host> {
    Arc::new(NativeHost {
        spawner: TokioSpawner,
        clock: SystemClock,
        rng: OsRngProvider,
        transport: NativeTransport,
        listener: NativeListener,
        doc_storage: NativeDocStorage::new(&storage_path),
        blob_storage: NativeBlobStorage::new(&storage_path),
        snapshot_storage: NativeSnapshotStorage::new(&storage_path),
        filesystem: NativeFilesystem::new(),
        tls: NativeTlsProvider,
    })
}
