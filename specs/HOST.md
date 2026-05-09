# HOST.md — Platform-Abstraction Contract

> Normative spec. See [SPEC.md § Conformance language](./SPEC.md#conformance-language)
> for RFC 2119 keyword usage.

This document specifies the `Host` trait surface — the contract between
the agentsync engine and the platform it runs on. It is the integration
seam for porting agentsync to a new runtime (browser, Deno, Tauri,
Obsidian plugin, embedded device).

The traits described here are exposed by the reference Rust crate as
`agentsync_core::host::*`. A reimplementation in another language
**SHOULD** preserve the same factoring even if the trait names differ,
because the engine's portability story depends on every "platform"
concern being injected through these traits rather than imported as a
global.

---

## 1. Overview

A `Host` is the bundle of platform capabilities a `Vault` needs. There
are nine sub-capabilities:

| Capability | Trait | Required? |
|---|---|---|
| Async runtime | `Spawner` | yes |
| Wall-clock + timers | `Clock` | yes |
| Cryptographic RNG | `Rng` | yes |
| Outbound transport | `Transport` | yes |
| Inbound listener | `Listener` | hub only (`Option`) |
| Document storage | `DocStorage` | yes |
| Blob storage | `BlobStorage` | yes |
| Snapshot index storage | `SnapshotStorage` | yes |
| Filesystem watch + I/O | `FilesystemAdapter` | optional (storage-only mode supported) |
| TLS cert provisioning | `TlsCertProvider` | hub-with-TLS only (`Option`) |
| Identity signing | `Signer` | yes (per-vault, not on `Host`) |

The `Host` itself is `Send + Sync + 'static`. All sub-traits are
`Send + Sync + 'static` unless noted.

```rust
pub trait Host: Send + Sync + 'static {
    fn spawner(&self) -> &dyn Spawner;
    fn clock(&self) -> &dyn Clock;
    fn rng(&self) -> &dyn Rng;
    fn transport(&self) -> &dyn Transport;
    fn listener(&self) -> Option<&dyn Listener>;
    fn doc_storage(&self) -> &dyn DocStorage;
    fn blob_storage(&self) -> &dyn BlobStorage;
    fn snapshot_storage(&self) -> &dyn SnapshotStorage;
    fn filesystem(&self) -> Option<&dyn FilesystemAdapter>;
    fn tls(&self) -> Option<&dyn TlsCertProvider>;
}
```

A reimplementation of any of these traits **MUST** satisfy every
contract clause in the relevant section below.

---

## 2. Runtime: `Spawner`, `Clock`

### 2.1 `Spawner`

```rust
pub trait Spawner: Send + Sync + 'static {
    fn spawn(&self, fut: BoxFuture<'static, ()>) -> SpawnHandle;
}
```

Contract:

- `spawn` **MUST** schedule the future for execution. It **MUST NOT**
  block the caller.
- The returned `SpawnHandle` **MAY** be dropped without affecting the
  task; the task continues to completion.
- `SpawnHandle` **MUST** support both `abort()` and `join()`:
  - `abort()` cancels the task. Cancellation is best-effort (the future
    may have already completed).
  - `join()` resolves when the task completes (whether normally or via
    abort). It **MUST NOT** panic.

The reference impl is `TokioSpawner` wrapping `tokio::spawn`. A wasm
impl **SHOULD** wrap `wasm_bindgen_futures::spawn_local` (in which
case the `?Send` bound on returned futures matters).

### 2.2 `Clock`

```rust
pub trait Clock: Send + Sync + 'static {
    fn now_ms(&self) -> i64;
    fn sleep(&self, d: Duration) -> BoxFuture<'static, ()>;
}
```

Contract:

- `now_ms()` returns wall-clock milliseconds since the Unix epoch
  (1970-01-01T00:00:00Z). Negative values **MUST** be supported (the
  spec is total) but are not produced under normal operation.
- `now_ms()` is used for `created_at` / `updated_at` timestamps and
  TLS cert validity windows. It is **not** authoritative for ordering
  — Automerge's change graph is.
- `sleep(d)` resolves after approximately `d` of real time. It **MAY**
  fire late on a busy runtime; callers **MUST NOT** rely on tight
  tolerance.

The reference impl is `SystemClock` using `std::time::SystemTime` and
`tokio::time::sleep`.

---

## 3. Crypto: `Rng`, `Signer`, `TlsCertProvider`

### 3.1 `Rng`

```rust
pub trait Rng: Send + Sync + 'static {
    fn fill_bytes(&self, buf: &mut [u8]);
}
```

Contract:

- `fill_bytes` **MUST** fill `buf` with cryptographically secure random
  bytes. A non-CSPRNG **MUST NOT** be used for this trait.
- Used for: nonce generation in the handshake, identity seed
  generation, UUID generation.

Reference impl: `OsRngProvider` wrapping `rand_core::OsRng`. A wasm
impl **SHOULD** delegate to `crypto.getRandomValues()`.

### 3.2 `Signer`

```rust
#[async_trait(?Send)]
pub trait Signer: Send + Sync + 'static {
    async fn sign(&self, msg: &[u8]) -> Result<[u8; 64]>;
    fn pubkey(&self) -> Pubkey;
}
```

Contract:

- `sign(msg)` returns an ed25519 signature over `msg` using the
  signer's private key. Returns exactly 64 bytes.
- `pubkey()` returns the corresponding ed25519 public key (32 bytes).
- The returned signature **MUST** verify against `pubkey()` for the
  same `msg` under standard ed25519 (RFC 8032).
- `sign` is async to accommodate external signers (ssh-agent, hardware
  tokens, WebAuthn). File-backed signers complete synchronously and
  **MAY** return immediately resolved futures.
- A signer **MAY** error if the user cancels (e.g., a Touch ID prompt).
  The returned `Error::Auth(message)` **SHOULD** identify the failure
  mode.

A `Signer` is **per-vault**, not part of `Host` — different vaults may
use different identities. The reference's `IdentitySigner` wraps
either a file-backed or ssh-agent identity.

### 3.3 `TlsCertProvider`

```rust
#[async_trait(?Send)]
pub trait TlsCertProvider: Send + Sync + 'static {
    async fn load_or_generate(&self, dir: &Path) -> Result<TlsCert>;
}

pub struct TlsCert {
    pub cert_der: Vec<u8>,
    pub key_der: Vec<u8>,
}
```

Contract:

- If `<dir>/tls.crt` and `<dir>/tls.key` both exist, load and return
  them.
- Otherwise, generate a fresh self-signed ed25519 keypair (10-year
  validity), persist atomically (write-tmp + rename), and return.
  See [STORAGE.md § 5](./STORAGE.md#tls-material-hub-only) for file
  format and mode requirements.
- Returned `cert_der` and `key_der` **MUST** be DER-encoded.
  `key_der` **MUST** be PKCS#8 (so rustls can consume it).

This trait is hub-only. `Host::tls()` returns `None` for browser hosts
where TLS is the underlying socket's concern.

---

## 4. Transport: `Transport`, `Conn`, `Listener`, `Acceptor`

### 4.1 `Transport`

```rust
#[async_trait(?Send)]
pub trait Transport: Send + Sync + 'static {
    async fn connect(&self, url: &str, opts: ConnectOpts) -> Result<Box<dyn Conn>>;
}

pub struct ConnectOpts {
    pub expected_hub_pubkey: Option<Pubkey>,
}
```

Contract:

- `connect` opens a connection to `url` (`wss://...` or `ws://...`)
  and returns a duplex frame channel.
- The transport **SHOULD** apply normal connect timeouts internally;
  the engine does not impose one.
- `expected_hub_pubkey` is informational — the transport itself does
  not enforce it; the engine's handshake does. It is provided in case
  the transport wants to log or pre-pin (e.g., for diagnostic UI).

### 4.2 `Conn`

```rust
#[async_trait(?Send)]
pub trait Conn: Send + 'static {
    async fn send(&mut self, frame: Bytes) -> Result<()>;
    async fn recv(&mut self) -> Result<Option<Bytes>>;
    fn channel_binding(&self) -> Option<[u8; 32]>;
    async fn close(self: Box<Self>) -> Result<()>;
}
```

Contract:

- `send(frame)` sends a binary WebSocket frame containing exactly the
  bytes of one MessagePack-encoded `Frame`. Errors are unrecoverable —
  the connection is dead.
- `recv()` returns `Ok(Some(bytes))` for the next received frame,
  `Ok(None)` if the peer closed cleanly, or `Err(_)` on protocol
  failure.
- `channel_binding()` returns the SHA-256 of the peer's TLS cert DER
  (32 bytes), or `None` if the underlying transport cannot expose it
  (plain `ws://`, browser `WebSocket`). Used by the handshake to
  enforce channel binding (see [WIRE.md § 4.5](./WIRE.md#45-channel-binding)).
  A `Conn` that returns `None` puts the engine in degraded
  channel-binding mode.
- `close()` performs a clean WebSocket close. It is best-effort; an
  error means the close didn't complete cleanly but the peer **SHOULD**
  still treat the connection as dead.

### 4.3 `Listener`

```rust
#[async_trait(?Send)]
pub trait Listener: Send + Sync + 'static {
    async fn bind(
        &self,
        addr: SocketAddr,
        tls: Option<TlsConfig>,
    ) -> Result<Box<dyn Acceptor>>;
}

pub struct TlsConfig {
    pub cert_der: Vec<u8>,
    pub key_der: Vec<u8>,
}
```

Contract:

- `bind` binds a TCP listener to `addr` and returns an `Acceptor`.
- If `tls` is `Some`, the listener performs TLS termination on each
  inbound connection using the supplied cert/key. If `None`, plaintext
  WebSocket only.
- Returning from `bind` **MUST** mean the listener is fully bound and
  ready to accept connections — there must be no race where a peer's
  immediate `connect` fails because the listener is still starting.

`Host::listener()` returns `None` for browser hosts (browsers cannot
bind listeners).

### 4.4 `Acceptor`

```rust
#[async_trait(?Send)]
pub trait Acceptor: Send + 'static {
    async fn accept(&mut self) -> Result<Option<Box<dyn Conn>>>;
    fn local_addr(&self) -> SocketAddr;
    async fn close(self: Box<Self>) -> Result<()>;
}
```

Contract:

- `accept()` blocks until a new inbound connection's WebSocket upgrade
  completes, then returns the `Conn`. Returns `Ok(None)` after
  `close()` has been called.
- `local_addr()` returns the actually-bound socket address (useful
  when `bind` used port `0`).
- `close()` stops accepting *new* connections. In-flight `Conn`s are
  unaffected and **MUST** be torn down by their owning peer task.

---

## 5. Storage: `DocStorage`, `BlobStorage`, `SnapshotStorage`

These three traits abstract the on-disk layout specified in
[STORAGE.md](./STORAGE.md). A reimplementation that targets a
non-filesystem backend (OPFS, IndexedDB, S3) implements these traits
against that backend.

### 5.1 `DocStorage`

```rust
#[async_trait(?Send)]
pub trait DocStorage: Send + Sync + 'static {
    async fn load(&self) -> Result<Option<Vec<u8>>>;
    async fn save(&self, bytes: &[u8]) -> Result<()>;
    async fn ensure_ready(&self) -> Result<()>;
}
```

Contract:

- `load()` returns the saved document bytes, or `Ok(None)` if no save
  has ever happened.
- `save(bytes)` replaces the stored document atomically. After `save`
  returns `Ok`, a subsequent `load()` **MUST** return `Some(bytes)`
  even after a crash.
- `ensure_ready()` performs any first-time setup (creating directories,
  initializing a database). It **MUST** be idempotent.

Atomicity for the filesystem reference is write-tmp + rename of
`doc.bin` (see [STORAGE.md § 2.3](./STORAGE.md#23-atomic-write)). A
non-filesystem reimplementation **MUST** provide an equivalent
all-or-nothing replacement.

### 5.2 `BlobStorage`

```rust
#[async_trait(?Send)]
pub trait BlobStorage: Send + Sync + 'static {
    async fn has(&self, hash: &str) -> bool;
    async fn get(&self, hash: &str) -> Result<Vec<u8>>;
    async fn put(&self, bytes: &[u8]) -> Result<String>;
    async fn put_with_hash(&self, hash: &str, bytes: &[u8]) -> Result<()>;
    async fn ensure_ready(&self) -> Result<()>;
}
```

Contract:

- All hashes are lowercase hexadecimal SHA-256 (64 chars).
- `has(hash)` returns whether the blob is locally available.
- `get(hash)` returns the blob's bytes; errors if absent.
- `put(bytes)` computes the hash, stores the blob, returns the hash.
- `put_with_hash(hash, bytes)` is for when the hash is supplied
  externally (e.g., from a wire frame). The implementation **MUST**
  verify `SHA-256(bytes) == hash` and reject on mismatch with
  `Error::Other("blob hash mismatch...")`.
- `ensure_ready()` initializes (creates directories) idempotently.

Writes **MUST** be atomic in the same sense as `DocStorage::save`.

### 5.3 `SnapshotStorage`

```rust
#[async_trait(?Send)]
pub trait SnapshotStorage: Send + Sync + 'static {
    async fn read(&self) -> Result<Vec<SnapshotEntry>>;
    async fn write(&self, entries: &[SnapshotEntry]) -> Result<()>;
    async fn ensure_ready(&self) -> Result<()>;
}

pub struct SnapshotEntry {
    pub label: String,
    pub heads: Vec<ChangeHash>,
    pub created_at_ms: i64,
}
```

Contract:

- `read()` returns the cached label index. Returns an empty `Vec` if
  no index exists yet (NOT an error).
- `write(entries)` replaces the index atomically.
- The on-disk format for the filesystem reference is JSON with
  base64-no-pad heads — see [STORAGE.md § 3](./STORAGE.md#snapshotsindexjson).
  A non-filesystem reimplementation **MAY** use any format internally
  but **MUST** preserve the data.

This is a *cache* of the document's `labels` map. A reimplementation
that does not provide cached fast-reads **MAY** make `read()` and
`write()` no-ops (the engine is correct without the cache).

---

## 6. Filesystem: `FilesystemAdapter`, `Watcher`

```rust
#[async_trait(?Send)]
pub trait FilesystemAdapter: Send + Sync + 'static {
    async fn read(&self, path: &Path) -> Result<Vec<u8>>;
    async fn write(&self, path: &Path, content: &[u8]) -> Result<()>;
    async fn delete(&self, path: &Path) -> Result<()>;
    async fn list(&self, path: &Path) -> Result<Vec<DirEntry>>;
    async fn exists(&self, path: &Path) -> bool;
    async fn hash(&self, path: &Path) -> Result<String>;
    async fn create_dir_all(&self, path: &Path) -> Result<()>;
    async fn remove_dir(&self, path: &Path) -> Result<()>;
    fn watch(
        &self,
        path: &Path,
        sink: UnboundedSender<FsEvent>,
    ) -> Result<Box<dyn Watcher>>;
}

pub struct DirEntry {
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: u64,
}

pub trait Watcher: Send + Sync {}

pub enum FsEvent {
    Touched(PathBuf),
    Removed(PathBuf),
    Renamed { from: PathBuf, to: PathBuf },
}
```

Contract per method:

- `read(path)` reads the entire file into memory.
- `write(path, content)` writes the file. Reimplementations **SHOULD**
  use atomic write (write-to-tmp + rename) to avoid mid-write states
  visible to the watcher.
- `delete(path)` removes the file; error if it does not exist.
- `list(path)` enumerates direct children. The order is unspecified.
- `exists(path)` returns presence (file or directory).
- `hash(path)` returns lowercase hex SHA-256 of the file's content.
- `create_dir_all(path)` creates the directory and any missing
  ancestors. Idempotent — succeeds if the directory already exists.
- `remove_dir(path)` removes an empty directory. Errors on a non-empty
  directory.
- `watch(path, sink)` installs a recursive watcher rooted at `path`.
  Events flow into `sink` until the returned `Watcher` is dropped.

The `Watcher` trait is just a marker — its only contract is that
dropping it stops the watch. Implementations **SHOULD** implement
`Drop` to release any underlying resources.

`FsEvent` ordering follows the underlying OS watcher's ordering. The
engine treats events as advisory and reconciles against the document.

`Host::filesystem()` returning `None` puts the engine in *storage-only
mode* — no filesystem materialization, no watcher. This is the
expected configuration for a browser app that holds the vault in OPFS
without binding to any user directory.

---

## 7. Wiring

### 7.1 Native factory

The reference exposes a single factory:

```rust
pub fn native_host(storage_path: PathBuf) -> Arc<dyn Host>;
```

`storage_path` is the `.agentsync/` directory. The factory wires up:

- `TokioSpawner`, `SystemClock`, `OsRngProvider` (stateless)
- `NativeDocStorage`, `NativeBlobStorage`, `NativeSnapshotStorage`
  (each holds `storage_path`)
- `NativeFilesystem` (wraps the existing `NodeFsAdapter` for now)
- `NativeTransport`, `NativeListener` (placeholders pending the
  cutover from the legacy `net` module)
- `NativeTlsProvider`

A reimplementation **SHOULD** provide an analogous single
factory-by-path entry point.

### 7.2 Hosts that aren't native

A wasm/browser host implements only the subset the runtime supports:

| Trait | Browser | Node | Native |
|---|---|---|---|
| `Spawner` | `wasm_bindgen_futures` | `tokio` (or vanilla) | `tokio` |
| `Clock` | `Date.now()` + `setTimeout` | `Date.now()` + `setTimeout` | `SystemTime` + `tokio::time::sleep` |
| `Rng` | `crypto.getRandomValues` | `crypto.getRandomValues` | `OsRng` |
| `Transport` | `WebSocket` | `ws` package | `tokio-tungstenite` |
| `Listener` | None | `ws` server | `tokio-tungstenite` |
| `DocStorage` | OPFS | `node:fs` | `tokio::fs` |
| `BlobStorage` | OPFS | `node:fs` | `tokio::fs` |
| `SnapshotStorage` | OPFS | `node:fs` | `tokio::fs` |
| `FilesystemAdapter` | None or FSAA | `node:fs` + `chokidar` | `notify` + `tokio::fs` |
| `TlsCertProvider` | None | `rcgen` (if hub) | `rcgen` |

A trait that is `None`-able on `Host` (i.e., `listener`, `filesystem`,
`tls`) **MAY** be skipped. A required trait **MUST** be provided.

### 7.3 Current state of the reference

As of this spec, the wasm crate (`crates/agentsync-wasm`) does **not**
yet implement `Host`. It exposes lower-level primitives (the `Doc`,
`Identity`, `Pubkey`, `SyncState` types and frame codec) directly, and
the TypeScript SDK at `sdks/typescript/` assembles those plus
JS-implemented adapters into a working `Vault` class — see
[API-TS.md](./API-TS.md). The Host trait surface is currently used only
on native builds.

This is a transitional state. A reimplementation that builds on top of
the wasm crate today **SHOULD** plan for the wasm `Host` to become
real, at which point the TypeScript SDK will route through it.

---

## 8. Async-runtime portability

Every async trait method uses `#[async_trait(?Send)]`. The `?Send`
relaxation matters because:

- Native futures are typically `Send` (tokio is multi-threaded).
- Wasm futures carrying `JsValue` are **not** `Send` (no real threads).

A reimplementation that wraps non-`Send` types in any of these futures
**MUST** preserve the `?Send` relaxation. A wrapper that requires
`Send` (e.g., re-imposing `async_trait` without `?Send`) breaks wasm.

---

## 9. Conformance

A `Host` implementation is conformant if:

1. Each implemented trait satisfies every "MUST" in its section.
2. Method signatures match exactly (ignoring async-runtime sugar that
   compiles to the same thing).
3. The atomicity guarantees on `DocStorage::save`, `BlobStorage::put*`,
   and `SnapshotStorage::write` are preserved.
4. Optional capabilities (`listener`, `filesystem`, `tls`) are reported
   honestly via `Option`.

A reimplementation in another language **MAY** rename or rebundle
traits, but the contract clauses must be preserved verbatim.

---

## 10. Cross-references

- [SPEC.md](./SPEC.md) — overall architecture and crate layout.
- [WIRE.md](./WIRE.md) — uses `Transport` and `Conn`.
- [STORAGE.md](./STORAGE.md) — what the storage traits persist.
- [DOCUMENT.md](./DOCUMENT.md) — what `DocStorage` ultimately stores.
- [AUTH.md](./AUTH.md) — how `Signer` is used in the handshake.
- [API-RUST.md](./API-RUST.md), [API-TS.md](./API-TS.md) — public APIs
  layered on top of `Host`.
