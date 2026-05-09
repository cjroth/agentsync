# API-RUST.md — Rust Public API

> Normative for the published Rust API. See [SPEC.md § Conformance
> language](./SPEC.md#conformance-language).

This document specifies the public API of the `agentsync-core` Rust
crate. It is the surface a Rust application sees when it adds
`agentsync-core` as a dependency, and the contract that must be
preserved across minor versions.

For the *byte-level* semantics underlying these methods (wire format,
on-disk format, document schema), see the linked specs. This document
states the API shape and the behaviors it guarantees, without
re-stating those semantics.

---

## 1. Crate-level layout

```
agentsync_core::
    Vault, OpenOptions, CreateOptions, CreatedVault, VaultId,
    VaultEvent, VaultEventKind, ReconnectOptions, VaultConfig,
    SyncHandle,                    // trait

    Doc, FileId, FileKind, FileMeta, DirectoryMeta, Label,
    SCHEMA_VERSION, content_hash,

    Identity, Pubkey, PUBKEY_LEN, SIGNATURE_LEN,

    Frame, HelloOp,

    Error, Result,

    AuthorizedPeer, parse_authorized_keys, render_authorized_keys,
    parse_peers_md, render_peers_md, PEERS_FILE,

    HANDSHAKE_DOMAIN, NONCE_LEN, build_transcript, random_nonce,

    AUTHORIZED_KEYS_FILE,
    DEFAULT_PORT, DEFAULT_LISTEN_ADDR, DEFAULT_LISTEN_ADDR_NO_TLS,
    USER_IDENTITY_FILENAME, USER_STATE_DIR,
    normalize_rendezvous_url, normalize_with_scheme,

    // native-only:
    BindOptions, Binding, NodeFsAdapter,
    discover_vault_id, agent_list_identities_at,

    // module:
    pub mod host;                  // see HOST.md, native-only
```

### 1.1 Conditional compilation

Items marked "native-only" are gated by `#[cfg(not(target_arch = "wasm32"))]`.
A wasm consumer **MUST NOT** import these — they will fail to compile.

A reimplementation **SHOULD** preserve this gate so the crate compiles
on the wasm target.

---

## 2. Versioning policy

The crate follows semver. For the public surface specified here:

- A change to a method **signature** (parameters, return type, async
  vs sync) is a **breaking** change.
- Adding a new public item is a **minor** change.
- Adding a new variant to a public `enum` is a **breaking** change
  unless the enum is `#[non_exhaustive]`. The current `Error` enum is
  not `#[non_exhaustive]` but **SHOULD** be — adding variants is
  expected.
- Changing the documented behavior of a method (e.g., when it errors)
  is a **breaking** change.

A reimplementation in another language **MUST** treat each public type
and method described below as part of its API contract.

---

## 3. `Vault` (native-only)

The top-level type. A `Vault` represents one open vault: a loaded
Automerge document, an `Identity`, optional network connections,
optional filesystem binding.

### 3.1 Construction

```rust
impl Vault {
    pub async fn create(opts: CreateOptions) -> Result<(Self, CreatedVault)>;
    pub async fn open(opts: OpenOptions)     -> Result<Self>;
}

pub struct CreateOptions {
    pub rendezvous_url: Option<String>,
    pub identity:       Option<Identity>,
    pub storage_path:   PathBuf,
}

pub struct OpenOptions {
    pub rendezvous_url: Option<String>,
    pub vault_id:       VaultId,
    pub identity:       Identity,
    pub storage_path:   PathBuf,
    pub hub_pubkey:     Option<Pubkey>,
    pub name:           Option<String>,
}

pub struct CreatedVault {
    pub vault_id: VaultId,
    pub identity: Identity,
}

pub type VaultId = String;
```

`create`:

- Generates a new `vault_id` (UUID v4).
- Initializes a fresh Automerge document per [DOCUMENT.md](./DOCUMENT.md).
- If `identity` is `None`, generates a new ed25519 keypair and persists
  it per [STORAGE.md § 8](./STORAGE.md#identity-files).
- Writes `.agentsync/config.toml` with the provided fields.
- Initializes `authorized_keys` containing the creator's pubkey.

`open`:

- Loads `.agentsync/doc.bin` into memory.
- Validates that the document's `vault_id` matches `opts.vault_id`.
- Does **not** automatically connect; call `connect()`.

Both methods **MUST** be safe to call concurrently for *different*
vaults; the same vault **MUST NOT** be opened twice in the same
process.

### 3.2 Accessors

```rust
impl Vault {
    pub fn id(&self) -> &VaultId;
    pub fn identity(&self) -> &Identity;
    pub fn pubkey(&self) -> Pubkey;
    pub fn storage_path(&self) -> &Path;
    pub fn name(&self) -> Option<&str>;
    pub fn subscribe(&self) -> broadcast::Receiver<VaultEvent>;
}
```

`subscribe` returns a fresh receiver of the vault's event stream; see
§ 3.7.

### 3.3 File operations

```rust
impl Vault {
    pub async fn write_text_file(&self, path: &str, content: &str) -> Result<()>;
    pub async fn read_text_file (&self, path: &str)                 -> Result<String>;
    pub async fn delete_file    (&self, path: &str)                 -> Result<()>;
    pub async fn rename_file    (&self, from: &str, to: &str)       -> Result<()>;
    pub async fn list_files     (&self)                             -> Result<Vec<FileMeta>>;
    pub async fn list_file_paths(&self)                             -> Result<Vec<String>>;
    pub async fn file_exists    (&self, path: &str)                 -> bool;
    pub async fn file_hash      (&self, path: &str)                 -> Result<String>;
}
```

All paths **MUST** be POSIX-normalized per
[DOCUMENT.md § 5](./DOCUMENT.md#path-normalization). Implementations
normalize on input and return normalized strings on output.

`list_files` returns only live (non-soft-deleted) entries. Order is
unspecified.

`file_hash` returns lowercase hex SHA-256 of the file's content,
matching the `content_hash` free function.

### 3.4 Directory operations

```rust
impl Vault {
    pub async fn create_directory   (&self, path: &str) -> Result<()>;
    pub async fn delete_directory   (&self, path: &str, recursive: bool) -> Result<()>;
    pub async fn rename_directory   (&self, from: &str, to: &str) -> Result<()>;
    pub async fn list_directories   (&self) -> Result<Vec<DirectoryMeta>>;
}
```

`delete_directory(path, recursive=true)` is a single Automerge
transaction (see [DOCUMENT.md § 3.3](./DOCUMENT.md#recursive-delete)).
With `recursive=false`, the operation **MUST** fail with
`Error::AlreadyExists` (or similar) if the directory has live
children.

### 3.5 History and labels

```rust
impl Vault {
    pub async fn create_label    (&self, name: &str)            -> Result<()>;
    pub async fn delete_label    (&self, name: &str)            -> Result<()>;
    pub async fn list_labels     (&self)                        -> Result<Vec<Label>>;
    pub async fn restore_label   (&self, name: &str)            -> Result<()>;
    pub async fn restore_to_heads(&self, heads: &[ChangeHash])  -> Result<()>;
    pub async fn restore_to_time (&self, target_ms: i64)        -> Result<()>;
}
```

Restoration semantics:

- Restoration is **additive**: it produces forward-going Automerge
  changes that bring the document state to match the past state. It
  **MUST NOT** rewrite history.
- `restore_to_heads` restores to the document state at the given heads.
- `restore_to_time(target_ms)` walks the change graph and restores to
  the state at the latest heads whose timestamps are `<= target_ms`.
  Wall-clock timestamps are advisory and may be skewed across peers
  (typically by milliseconds with NTP); precision is best-effort.
- `restore_label(name)` is shorthand for `restore_to_heads(label.heads)`.

### 3.6 Networking

```rust
impl Vault {
    pub async fn connect    (&mut self) -> Result<()>;
    pub async fn disconnect (&mut self);
    pub async fn connect_with_reconnect(&mut self, opts: ReconnectOptions) -> Result<()>;
    pub async fn listen     (&mut self, addr: SocketAddr) -> Result<SocketAddr>;
    pub async fn listen_plain(&mut self, addr: SocketAddr) -> Result<SocketAddr>;
    pub async fn unlisten   (&mut self);
    pub async fn peer_count (&self) -> usize;
    pub async fn authorized_pubkeys(&self) -> Vec<Pubkey>;
}

pub struct ReconnectOptions {
    pub max_attempts:    u32,
    pub initial_backoff: Duration,
    pub max_backoff:     Duration,
}
```

`connect` performs the four-message handshake against the configured
`rendezvous_url`. See [WIRE.md § 4](./WIRE.md#handshake-normative).

`listen` binds a TLS WebSocket listener on `addr`. The hub generates a
self-signed cert if needed (see [STORAGE.md § 5](./STORAGE.md#tls-material-hub-only)).
Returns the actually-bound `SocketAddr` (useful for port `0`).

`listen_plain` binds plain `ws://`. **SHOULD** be used only behind a
TLS-terminating reverse proxy; implementations **SHOULD** print a
warning when used otherwise.

`connect_with_reconnect` retries `connect` with exponential backoff
between `initial_backoff` and `max_backoff`. It **MUST** abort on
`Error::Auth` (the credentials are wrong; retrying won't help).

### 3.7 Event stream

```rust
pub enum VaultEventKind {
    Connected,
    Disconnected,
    FileChanged { path: String },
    SyncProgress { percent: u8 },
    Error(String),
}

pub struct VaultEvent {
    pub kind: VaultEventKind,
}
```

`subscribe` returns a `tokio::sync::broadcast::Receiver<VaultEvent>`.
Multiple consumers **MAY** subscribe; each gets its own receiver.

Note: the TypeScript SDK exposes a richer event enum (with hub pubkey
and reason fields). This is intentional — the TS SDK orchestrates the
state machine itself; the Rust crate's enum is the underlying minimal
set. See [API-TS.md § 5](./API-TS.md#5-events).

### 3.8 Filesystem binding

```rust
pub struct BindOptions {
    pub exclude_patterns:    Vec<String>,
    pub include_patterns:    Vec<String>,
    pub attachment_max_bytes: u64,
    pub text_file_max_bytes:  u64,
}

impl Vault {
    pub async fn bind_directory(&mut self, path: &Path, opts: BindOptions)
        -> Result<Arc<Binding>>;
    pub async fn binding_arc(&self) -> Option<Arc<Binding>>;
    pub async fn materialize(&self, binding: &Arc<Binding>) -> Result<()>;
}
```

`bind_directory` installs a filesystem watcher over `path` and starts
mirroring document changes to disk and disk changes to the document.
`Binding` owns the watcher; dropping the `Arc` does not stop the bind
unless all references are gone.

`materialize` writes every live file in the document out to the bound
directory. **MUST** be idempotent.

### 3.9 Lifecycle

```rust
impl Vault {
    pub async fn flush(&self)        -> Result<()>;
    pub async fn close(self)         -> Result<()>;
    pub fn notify_doc_changed(&self);
    pub async fn debug_dump(&self)   -> Result<Vec<(String, Option<String>)>>;
}
```

`flush` forces a save of `doc.bin` and the snapshot index. **SHOULD**
be called before shutdown to ensure no acknowledged changes are lost.

`close` consumes the `Vault`, performs a final flush, disconnects, and
unbinds.

`notify_doc_changed` is a hint to the engine that the document has
changed and any pending sync messages should be regenerated. Used by
adapters that mutate the document outside the normal API path.

`debug_dump` returns `(path, body)` pairs for diagnostic introspection.
**MUST NOT** be relied on for normal operation — its format may change.

---

## 4. `Doc`

The `Doc` type wraps an `automerge::AutoCommit` and exposes the
document operations defined in [DOCUMENT.md](./DOCUMENT.md). It is
available on **both** native and wasm targets.

```rust
impl Doc {
    pub fn new(vault_id: &str)     -> Result<Self>;
    pub fn load(bytes: &[u8])      -> Result<Self>;
    pub fn save(&mut self)         -> Vec<u8>;
    pub fn save_incremental(&mut self) -> Vec<u8>;
    pub fn fork(&mut self)         -> Self;

    pub fn vault_id(&mut self)     -> Result<String>;
    pub fn heads(&mut self)        -> Vec<ChangeHash>;

    pub fn merge(&mut self, other: &mut Doc) -> Result<bool>;
    pub fn generate_sync_message(&mut self, state: &mut amsync::State)
        -> Option<Vec<u8>>;
    pub fn receive_sync_message(&mut self, state: &mut amsync::State, msg: &[u8])
        -> Result<bool>;
}
```

File operations on `Doc`:

```rust
impl Doc {
    pub fn write_text_file(&mut self, path: &str, content: &str) -> Result<FileId>;
    pub fn read_file      (&mut self, path: &str)                -> Result<String>;
    pub fn file_exists    (&mut self, path: &str)                -> bool;
    pub fn file_hash      (&mut self, path: &str)                -> Result<String>;
    pub fn delete_file    (&mut self, path: &str)                -> Result<()>;
    pub fn rename_file    (&mut self, from: &str, to: &str)      -> Result<()>;
    pub fn list_files     (&mut self)                            -> Result<Vec<FileMeta>>;
    pub fn list_file_paths(&mut self)                            -> Result<Vec<String>>;
    pub fn find_file_by_path(&mut self, path: &str)              -> Result<Option<FileId>>;
    pub fn read_file_meta (&mut self, fid: &str)                 -> Result<Option<FileMeta>>;
    pub fn write_attachment(&mut self, path: &str, hash: &str, size: i64)
        -> Result<FileId>;
}
```

Directory operations on `Doc`: same shape as on `Vault`, prefixed
without the `_directory`/`_file` modifier (see source for exact names).

Label operations on `Doc`:

```rust
impl Doc {
    pub fn create_label    (&mut self, label: &str)              -> Result<()>;
    pub fn delete_label    (&mut self, label: &str)              -> Result<()>;
    pub fn list_labels     (&mut self)                           -> Result<Vec<Label>>;
    pub fn get_label_heads (&mut self, label: &str)              -> Result<Vec<ChangeHash>>;
    pub fn restore_to_heads(&mut self, heads: &[ChangeHash])     -> Result<()>;
    pub fn restore_to_time (&mut self, target_ms: i64)           -> Result<()>;
}
```

`Doc` is the layer used by the wasm bridge (`agentsync-wasm`) and is
available without any of the OS-touching machinery (`Vault`, `host`,
`fs`, `net`).

---

## 5. `Identity` and `Pubkey`

```rust
pub struct Identity { /* opaque */ }

impl Identity {
    pub fn generate() -> Self;
    pub fn from_seed(seed: [u8; 32]) -> Self;
    pub fn seed(&self) -> Result<[u8; 32]>;
    pub fn pubkey(&self) -> Pubkey;
    pub async fn sign(&self, message: &[u8]) -> Result<[u8; 64]>;
}
```

There are two construction modes (file-backed and ssh-agent-backed),
exposed via free functions in the crate root and via
`Identity::load_from_file` / similar (see source). Both produce an
`Identity` whose `sign(...)` produces a standard ed25519 signature.

```rust
pub struct Pubkey([u8; 32]);

impl Pubkey {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self>;
    pub fn from_ssh_string(s: &str) -> Result<Self>;
    pub fn to_ssh_string(&self) -> String;
    pub fn fingerprint_sha256(&self) -> String;
    pub fn as_bytes(&self) -> &[u8; 32];
    pub fn verify(&self, message: &[u8], sig: &[u8]) -> bool;
}

pub const PUBKEY_LEN: usize    = 32;
pub const SIGNATURE_LEN: usize = 64;
```

The SSH wire format is fixed at `ssh-ed25519 <base64>` per
[STORAGE.md § 7.4](./STORAGE.md#ssh-wire-format).

---

## 6. Wire types

```rust
pub enum HelloOp { Join, Create }

pub enum Frame {
    HelloHub  { vault_id: String, hub_identity_pubkey: Vec<u8>,
                hub_nonce: Vec<u8>, tls_cert_fingerprint: Vec<u8>,
                vault_name: Option<String> },
    HelloPeer { peer_identity_pubkey: Vec<u8>, peer_nonce: Vec<u8>,
                op: HelloOp },
    ProofHub  { sig: Vec<u8> },
    ProofPeer { sig: Vec<u8> },
    Sync      { bytes: Vec<u8> },
    BlobFetch { hash: String },
    BlobPush  { hash: String, bytes: Vec<u8> },
    Ping      { ts: i64 },
    Pong      { ts: i64 },
    Error     { message: String },
}
```

Encoding is MessagePack with named tags; see [WIRE.md § 2](./WIRE.md#frame-format).

```rust
pub const HANDSHAKE_DOMAIN: &[u8] = b"agentsync-auth-v1"; // 17 bytes
pub const NONCE_LEN: usize = 32;

pub fn random_nonce() -> [u8; 32];
pub fn build_transcript(
    hub_nonce:            &[u8; 32],
    peer_nonce:           &[u8; 32],
    tls_cert_fingerprint: &[u8],         // 0 or 32 bytes
    hub_pubkey:           &[u8; 32],
    peer_pubkey:          &[u8; 32],
) -> Vec<u8>;
```

These helpers are exposed for clients implementing custom transports.

---

## 7. `authorized_keys` parsing

```rust
pub struct AuthorizedPeer {
    pub pubkey: Pubkey,
    pub label:  String,
}

pub fn parse_authorized_keys(content: &str)    -> Vec<AuthorizedPeer>;
pub fn render_authorized_keys(peers: &[AuthorizedPeer]) -> String;
```

Parser semantics: see [STORAGE.md § 7.2](./STORAGE.md#parser-rules).

The deprecated `peers.md` format is also exposed for migration:

```rust
pub const PEERS_FILE: &str = "peers.md";
pub fn parse_peers_md(content: &str) -> Vec<AuthorizedPeer>;
pub fn render_peers_md(peers: &[AuthorizedPeer]) -> String;
```

A reimplementation **MAY** omit the `peers.md` helpers; new vaults
**MUST NOT** create a `peers.md`.

---

## 8. Error type

```rust
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io: {0}")]              Io(#[from] std::io::Error),
    #[error("automerge: {0}")]       Automerge(#[from] automerge::AutomergeError),
    #[error("automerge load: {0}")]  AutomergeLoad(#[from] automerge::LoadChangeError),
    #[error("invalid path: {0}")]    InvalidPath(String),
    #[error("not found: {0}")]       NotFound(String),
    #[error("already exists: {0}")]  AlreadyExists(String),
    #[error("auth failed: {0}")]     Auth(String),
    #[error("config: {0}")]          Config(String),
    #[error("protocol: {0}")]        Protocol(String),
    #[error("network: {0}")]         Network(String),
    #[cfg(not(target_arch = "wasm32"))]
    #[error("notify: {0}")]          Notify(#[from] notify::Error),
    #[error("serde json: {0}")]      SerdeJson(#[from] serde_json::Error),
    #[error("msgpack encode: {0}")]  MsgpackEncode(#[from] rmp_serde::encode::Error),
    #[error("msgpack decode: {0}")]  MsgpackDecode(#[from] rmp_serde::decode::Error),
    #[error("websocket: {0}")]       WebSocket(String),
    #[error("vault: {0}")]           Vault(String),
    #[error("size limit exceeded: {0}")] TooLarge(String),
    #[error("invalid utf8")]         InvalidUtf8,
    #[error("{0}")]                  Other(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
```

Variant semantics:

| Variant | When |
|---|---|
| `Io`             | underlying file/socket I/O failure |
| `Automerge`      | Automerge operation failed (e.g., wrong type at a path) |
| `AutomergeLoad`  | `doc.bin` was unparseable |
| `InvalidPath`    | path failed normalization (see DOCUMENT.md § 5) |
| `NotFound`       | file/label does not exist |
| `AlreadyExists`  | create-when-exists, or non-recursive directory delete on non-empty |
| `Auth`           | signature invalid, peer not authorized, agent refused |
| `Config`         | malformed `config.toml` |
| `Protocol`       | wire-protocol violation (unexpected frame, bad encoding) |
| `Network`        | TCP/TLS/DNS issue distinct from I/O |
| `WebSocket`      | tungstenite-level failure |
| `TooLarge`       | exceeded `attachment_max_bytes` or `text_file_max_bytes` |
| `Vault`          | vault-level invariant violation |
| `Other`          | catch-all; **SHOULD** be replaced by a specific variant when possible |

Callers **MUST** be prepared for new variants; this enum is not
`#[non_exhaustive]` today but **SHOULD** be treated as if it were. New
variants will be added in minor versions.

---

## 9. `SyncHandle` trait

```rust
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
    async fn read_blob (&self, hash: &str) -> Result<Vec<u8>>;
    async fn write_blob(&self, hash: &str, bytes: &[u8]) -> Result<()>;
    async fn wait_doc_changed(&self);
    async fn authorized_pubkeys(&self) -> Vec<Pubkey>;
    async fn authorized_peers(&self)   -> Vec<AuthorizedPeer>;
    async fn disconnect_unauthorized_peers(&self, authorized: &[Pubkey]);
}
```

`SyncHandle` is the contract a network layer (the `net::client` and
`net::server` modules in the reference) talks to. It is exposed
publicly so a third party can build a custom transport while reusing
the engine.

A reimplementation **SHOULD** treat this trait as advanced — most
consumers should use `Vault` directly.

---

## 10. Constants and helpers

```rust
pub const SCHEMA_VERSION: i64 = 1;

pub const AUTHORIZED_KEYS_FILE: &str = "authorized_keys";

pub const DEFAULT_PORT: u16 = 443;
pub const DEFAULT_LISTEN_ADDR:        &str = "0.0.0.0:443";
pub const DEFAULT_LISTEN_ADDR_NO_TLS: &str = "0.0.0.0:80";

pub const USER_STATE_DIR:        &str = ".agentsync";
pub const USER_IDENTITY_FILENAME: &str = "id_ed25519";

pub fn normalize_rendezvous_url(url: &str) -> String;
pub fn normalize_with_scheme(url: &str)    -> String;

pub fn content_hash(bytes: &[u8]) -> String; // lowercase hex SHA-256
```

These are normative — a reimplementation **MUST** match them on the
wire and on disk. See [WIRE.md § 10](./WIRE.md#10-constants).

---

## 11. `host` module

The `host` module re-exports the trait surface specified in
[HOST.md](./HOST.md):

```rust
pub mod host {
    pub use crypto::{Rng, Signer, TlsCert, TlsCertProvider};
    pub use filesystem::{DirEntry, FilesystemAdapter, FsEvent, Watcher};
    pub use runtime::{Clock, SpawnHandle, SpawnHandleImpl, Spawner};
    pub use storage::{BlobStorage, DocStorage, SnapshotEntry, SnapshotStorage};
    pub use transport::{Acceptor, Conn, ConnectOpts, Listener, TlsConfig, Transport};
    // pub use native::native_host; // factory
}
```

Available only when `cfg!(not(target_arch = "wasm32"))`.

---

## 12. Stability summary

| API | Stability |
|---|---|
| `Vault` methods listed above | stable; semver-protected |
| `Doc` methods listed above   | stable; semver-protected |
| `Identity`, `Pubkey`, `Frame`, `HelloOp` | stable; semver-protected |
| `Error` enum                  | stable shape, expect new variants in minor versions |
| `host` traits                 | stable; semver-protected (see HOST.md) |
| `SyncHandle` trait            | stable but advanced; **SHOULD NOT** be implemented externally without coordination |
| `debug_dump`                  | unstable; format may change |
| Internal modules (`net::*` types not listed here) | unstable; not part of the public API |

A reimplementation in another language **MUST** preserve method names,
parameter order, and error semantics for items marked stable.

---

## 13. Cross-references

- [API-TS.md](./API-TS.md) — corresponding TypeScript surface.
- [DOCUMENT.md](./DOCUMENT.md) — schema operated on by `Doc`.
- [WIRE.md](./WIRE.md) — protocol used by `Vault::connect` /
  `Vault::listen`.
- [HOST.md](./HOST.md) — traits in the `host` module.
- [STORAGE.md](./STORAGE.md) — what `Vault::storage_path` contains.
