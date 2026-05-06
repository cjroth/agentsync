# agentsync — Product Spec (CLI, v1)

## What this is

A real-time sync engine for a directory of files, designed for AI agent workflows. Multiple agents and humans across multiple machines connect to the same vault and see the same directory state, with sub-second propagation and CRDT-based merge of concurrent edits.

The deliverables are:

1. A **Rust crate** (`agentsync-core`) that implements the sync engine.
2. A **CLI binary** (`agentsync`) that wraps the core. The same binary acts as both client and rendezvous peer (the "server" is just `agentsync --listen`).

A future TypeScript / WASM wrapper for use in browser environments and Obsidian plugins is explicitly v2 — out of scope for v1.

The E2E test suite is treated as a first-class deliverable. If a feature isn't covered by an E2E test that runs the real CLI binary in a multi-peer configuration, it doesn't ship.

---

## Goals (v1)

1. Bidirectional realtime sync of a local directory between peers running the same CLI binary, over websocket.
2. Multiple peers (CLI instances on different machines) connect to the same vault and converge on identical state, with concurrent edits merging cleanly via Automerge CRDTs.
3. TLS 1.3 for all peer-to-peer traffic. Auth via a shared vault key.
4. Point-in-time recovery to any moment in the document's history, using Automerge's native history primitives.
5. Named recovery points ("snapshots") as labels into the change graph.
6. CLI binary distributed as a single static executable for macOS, Linux, and Windows. Target binary size: under 15MB.
7. Vault key is a single piece of secret material the user manages; injected via env var or keyring.
8. Zero-infrastructure deployment: the "server" is `agentsync --listen` running on any reachable machine.

## Non-goals (v1)

- TypeScript / WASM SDK or Obsidian plugin. v2.
- End-to-end encryption above TLS. The rendezvous peer holds the vault contents in plaintext on its own filesystem; treat it as a trusted machine you control. E2EE with a relay-only rendezvous is a v2 feature.
- Multi-tenant SaaS hosting.
- Off-peer durability for snapshots and history. Everything lives on the peer that created it; users who want off-machine durability use any backup tool against `.agentsync/`. Native S3 push is v1.1.
- Web UI / dashboard.
- Mobile clients.
- Permission systems, RBAC, or per-file access control. A vault key grants full read/write to the entire vault.
- Key revocation. Lost or stolen keys cannot be revoked in v1.
- Conflict resolution UI.
- Partitioning, sharding, or per-agent directory isolation.
- Per-file or per-directory CRDT partitioning. v1 uses one Automerge document per vault.
- Binary file delta sync. Files are content-addressed and stored whole.

---

## Architecture

There is no separate backend service. Every agentsync instance is the same CLI binary running the same core library. One instance runs with `--listen` and acts as the always-on rendezvous peer; other instances connect to it as clients. The rendezvous peer is identical code — it just happens to be running on a machine with a public address and accepts incoming connections.

```
┌────────────────────────────────────────────────────────────┐
│                       Local Peer                            │
│                                                              │
│   Markdown files on disk                                     │
│        ↕ (notify crate / fsevents / inotify)                 │
│   ┌──────────────────────────────────────────────────────┐  │
│   │                agentsync-core (Rust)                  │  │
│   │                                                       │  │
│   │      FilesystemAdapter ↔ VaultEngine ↔ Net           │  │
│   │                          │                            │  │
│   │                  Automerge Document                   │  │
│   │                  (in memory, includes full history)   │  │
│   │                          ↕                            │  │
│   │                  .agentsync/ (on-disk state)          │  │
│   │                  - doc.bin (saved Automerge doc)      │  │
│   │                  - snapshots/index.json               │  │
│   │                  - blobs/<sha256> (attachments)       │  │
│   └──────────────────────────────────────────────────────┘  │
│                          ↕ wss:// (TLS 1.3)                  │
└──────────────────────────│──────────────────────────────────┘
                           │  Automerge sync messages
                           ↓
┌──────────────────────────────────────────────────────────────┐
│        Rendezvous Peer (agentsync --listen)                   │
│                                                                │
│   Identical CLI to local peer. Bound to a public address.      │
│   Holds the same .agentsync/ state on its own filesystem.      │
│   Fans out updates from one connected peer to all others.      │
└──────────────────────────────────────────────────────────────┘
                           ↑
                           │ other peers connect here
                           │
              ┌────────────┴────────────┐
              │                          │
        Other peer                 Other peer
        (laptop, agent             (CC Web sandbox,
         in cloud sandbox)          phone, etc.)
```

**Key properties:**

- The rendezvous peer is just another instance of the CLI. There is no separate backend codebase, no Postgres, no S3, no Docker Compose stack to deploy.
- Each vault is **one Automerge document**. Tree structure (directories, file metadata, paths) and file contents (text bodies as Automerge `Text`) live inside this one doc. Operations across files are atomic Automerge transactions.
- The Automerge document **already contains its full history** as a DAG of changes. We don't maintain a separate append log — the doc IS the log.
- All peer-to-peer traffic is TLS 1.3, using Automerge's built-in sync protocol over a thin websocket framing layer.
- Point-in-time recovery uses Automerge's native `*_at(heads)` API. Snapshots are saved sets of "heads" (change hashes) labeled with a name.

---

## Data model

### One Automerge document per vault

A vault corresponds to exactly one Automerge document. The document's root is a JSON-shaped object:

```rust
// Logical schema (Automerge values are typed but JSON-shaped)
{
  "schema_version": 1,
  "directories": Map<DirId, DirectoryMeta>,
  "files":       Map<FileId, FileEntry>,
  "labels":      Map<String, Vec<u8>>,  // label name → encoded heads
}

struct DirectoryMeta {
  path: String,        // POSIX, NFC-normalized
  created_at: i64,     // epoch ms
  deleted_at: Option<i64>,  // tombstone
}

struct FileEntry {
  meta: FileMeta,       // plain fields, edited via Automerge map mutations
  content: Text,        // for text files: Automerge Text (CRDT-merged string)
  binary_hash: Option<String>,  // for attachments: pointer into .agentsync/blobs/
}

struct FileMeta {
  path: String,         // POSIX, NFC-normalized
  kind: String,         // "text" | "attachment"
  size: i64,
  created_at: i64,
  updated_at: i64,
  deleted_at: Option<i64>,
}
```

Files and directories are identified by **stable UUIDs** (the keys of the maps), not paths. Renames are a single field update on `meta.path`. No delete-plus-create. Two files at the same path at different times are distinguishable (they have different IDs).

### Why one big Automerge doc

We initially considered splitting into a tree doc + per-file content docs, for memory bounding and lazy loading. We're choosing one big doc because:

- **PITR is unambiguous.** Automerge's `*_at(heads)` reads any past state of the entire vault atomically. With per-file docs, restoring would require coordinating heads across many docs.
- **Cross-file operations are atomic.** Folder rename, bulk delete, mass-move are single Automerge transactions.
- **Sync is simpler.** Automerge's sync protocol works on one document. Per-file docs would mean N sync states per peer connection.
- **Implementation is much smaller.** One doc to load, one doc to save, one set of heads to track.

The trade-off is that every peer holds the full Automerge document in memory. Automerge 3 reduced memory usage by ~10x via columnar compression at runtime, so this is much less of a concern than it was a year ago. For agent workflows (tens of MB of markdown), memory is a non-issue. If users hit scale issues, per-file partitioning is a v2 lever.

### Why Automerge over Yjs

Both are mature CRDTs. We're using Automerge because:

- **History is a first-class data structure.** Every change has a hash, every state is identified by a set of heads, and `*_at(heads)` reads the document at any past point. PITR is a single API call, not a custom log replay system.
- **JSON-shaped data model.** Our data (files, directories, metadata, paths) is fundamentally JSON-shaped. Automerge maps to this naturally; Yjs requires modeling everything as `Y.Map<Y.Map<Y.Text>>` which is awkward for non-text fields.
- **Built-in sync protocol.** Automerge ships a sync protocol with state vectors, incremental updates, and efficient catch-up. We use it directly instead of writing our own.
- **Native Rust.** Automerge's canonical implementation is Rust. We get full performance and full API surface, not a WASM wrapper.
- **Cross-language story for v2.** When we add the TypeScript/WASM wrapper, the Obsidian plugin, native iOS app, etc., they all use the same Automerge core. The vault format is portable.

For text-editor performance benchmarks, Yjs is faster. For our workload (agents writing whole files at a time, history-as-feature), Automerge fits better.

### Path normalization rules

- POSIX forward slashes only (translated on Windows).
- Case-sensitive.
- Unicode NFC normalization on all paths.
- Enforced at the core boundary.

### Directory handling

Directories exist as explicit entries in the `directories` map. This supports empty directories, directory metadata, and atomic directory operations. When a directory is deleted, its children are *not* automatically deleted — the SDK's `delete_directory(path, recursive: true)` wraps the cascade in an Automerge transaction so it's atomic.

Filesystem materialization handles directories implicitly (parents of every live file) and explicitly (entries in the `directories` map for empty dirs).

---

## Storage model

The Automerge document is the source of truth. It contains the full history of every change, compressed via Automerge's columnar encoding. Everything else is derivable from it.

### On-disk layout

```
my-vault/
├── notes/
│   ├── research.md              ← human-readable markdown
│   └── todo.md
├── README.md
└── .agentsync/                  ← managed entirely by the CLI
    ├── config.toml              ← vault id, rendezvous url, key source
    ├── doc.bin                  ← saved Automerge document (full history, compressed)
    ├── doc.bin.tmp              ← write-ahead temp file for atomic save
    ├── snapshots/
    │   └── index.json           ← named labels: { "v1": <heads>, ... }
    ├── blobs/
    │   └── <sha256>             ← raw bytes of binary attachments
    └── lock                     ← prevents two CLI processes binding the same vault
```

### doc.bin

A serialized Automerge document containing the entire change history. Written by `doc.save()`, loaded by `Automerge::load()`. The format is Automerge's native compressed binary encoding.

Persistence cadence:
- **Every change** is appended in-memory to the live document.
- **Every N seconds** (default: 1) or **every N changes** (default: 100), whichever first, the document is saved to `doc.bin.tmp` and atomically renamed to `doc.bin`. The save is incremental — Automerge's `save_incremental` appends only new changes since the last save, keeping the I/O small.
- **On clean shutdown**, a final save flush is performed.
- **On crash recovery**, the loaded `doc.bin` is the source of truth. Any changes that hadn't been flushed are lost — but they were also unacknowledged to other peers, so consistency is preserved.

### snapshots/index.json

A simple JSON file mapping human-readable labels to encoded Automerge heads:

```json
{
  "schema_version": 1,
  "labels": [
    { "label": "v1.0-shipped",          "heads": "base64(<change-hashes>)", "created_at": 1705000000000 },
    { "label": "before-bad-agent-run",  "heads": "base64(<change-hashes>)", "created_at": 1705100000000 }
  ]
}
```

Snapshots are not separate state blobs. They're just labeled pointers into the document's existing history. Restoring "v1.0-shipped" finds the heads, then uses Automerge's `*_at(heads)` to read state at that point.

The labels also live inside the Automerge doc itself (in the root `labels` map), so they sync between peers automatically. The `snapshots/index.json` file is just a local cache of that map for fast reads without loading the doc.

### blobs/

Binary attachments (PNGs, PDFs, etc.) are not stored in the Automerge doc. The file entry's `binary_hash` field points to a content-addressed blob in `.agentsync/blobs/<sha256>`. Blobs sync between peers as a separate protocol message (`BLOB_FETCH` / `BLOB_PUSH`) and are GC'd when no live file references them.

---

## Point-in-time recovery

PITR is a primary feature, not a bolt-on. Automerge's history primitives make it nearly trivial.

### Restoring to a past moment

Two flavors:

**1. Restore by heads** (precise, internal):

```rust
async fn restore_to_heads(&mut self, heads: &[ChangeHash]) -> Result<()> {
    // Pause the binding
    self.binding.pause().await?;
    
    // Read the full vault state at those heads
    let past_doc = self.doc.fork_at(heads)?;
    
    // Compute the diff from current to past, apply as new changes
    self.doc.merge(&past_doc)?;
    // Note: this does NOT undo changes since `heads`. To undo, we apply
    // the inverse of the difference, producing new forward-going changes
    // that bring the document to match `past_doc`'s state.
    self.apply_state_match(&past_doc).await?;
    
    self.binding.resume().await?;
    Ok(())
}
```

**2. Restore by timestamp** (user-facing):

Every Automerge change carries a timestamp. To restore to time T:

```rust
async fn restore_to_time(&mut self, target_ms: i64) -> Result<()> {
    // Walk the change graph; collect heads = changes whose timestamp ≤ target_ms
    let all_changes = self.doc.get_changes(&[])?;
    let target_heads = compute_heads_at_time(&all_changes, target_ms);
    self.restore_to_heads(&target_heads).await
}
```

The restore is **additive**: it produces new forward-going changes that bring the document state to match the past state, rather than rewriting history. Other peers see incoming Automerge changes that converge to the restored state, merged with any concurrent edits. This is the same additive-restore model we'd choose with any CRDT.

### Restore semantics across distributed peers

Automerge changes use logical clocks for causal ordering, but each change also carries a wall-clock timestamp. The two notions don't perfectly align in distributed systems: an update made at T=10 on peer A but received at peer B at T=15 is timestamped 10 (from A's clock) — but peer B might be skewed. Honesty: PITR by timestamp is approximate to within clock skew between peers (typically milliseconds with NTP).

This is documented and is fine for the use case.

### Snapshots as named recovery points

```bash
agentsync snapshot create "before-bad-run"
# Records: snapshots/index.json gets { "before-bad-run": <current_heads> }
# Also writes into the doc's labels map, so it syncs.

agentsync snapshot list
# Shows all labels with their wall-clock dates.

agentsync snapshot restore "before-bad-run"
# Equivalent to: agentsync restore-to-heads <heads_for_label>
```

A snapshot is essentially a tag — tiny (one entry in a map), instant to create, syncs with the doc.

There is no periodic snapshot scheduler. The Automerge document already contains the full history. Users (or agents) create labels at meaningful points.

### Compaction and retention

Automerge documents grow as history accumulates. Eventually you want to drop old history.

Automerge supports this via `bundle()` and `save()` with options to drop history before specified heads. `agentsync compact` does:

1. If `now - oldest_change_time < retention_days`, do nothing.
2. Otherwise, find the cutoff heads at `now - retention_days`.
3. Build a compacted document containing the state at the cutoff plus all changes after it.
4. Replace `doc.bin` atomically.

After compaction, PITR within the retention window still works. PITR before the cutoff is not possible — the granular history is gone. Snapshot labels pointing before the cutoff are pruned with a warning.

The default retention is 30 days, configurable via `[sync] log_retention_days`.

---

## Security and authentication

### Threat model

Three adversaries:

**1. Network adversary.** Mitigated by TLS 1.3.
- Confidentiality, authentication, forward secrecy.
- For post-quantum resistance, terminate TLS at a proxy that supports hybrid PQ key exchange (Caddy, Cloudflare, recent nginx). Deployment concern, not core concern.

**2. Compromised rendezvous peer.** Not mitigated.
- The rendezvous holds the vault key and plaintext content.
- Treat the rendezvous VM as a trusted machine you control. Same posture as your own laptop.

**3. Disk theft / backup theft.** Not mitigated by agentsync.
- Files on disk are plaintext (working directory is plaintext markdown by design).
- Use full-disk encryption (FileVault, LUKS, BitLocker) on every machine running agentsync.

### Authentication model

There is no concept of users, accounts, or ownership in v1. A vault is a standalone primitive identified by a UUID, and access is gated entirely by possession of the vault key.

The vault key is a 32-byte random secret generated by `agentsync init`. To derive an auth token:

```
auth_token = HMAC-SHA256(vault_key, "agentsync-auth-v1")
```

When a peer connects to the rendezvous it presents `(vault_id, auth_token)` in the websocket handshake. The rendezvous compares the token against its own derivation from the same vault key. Mismatch → connection rejected.

Anyone with the vault key has full access. The key *is* the credential.

When a hosted offering is built later, a `users` table and an `owners` join table can be added without changing the wire protocol or the on-disk vault format.

---

## Rendezvous peer

There is no separate backend codebase. The "server" is just `agentsync --listen` running on a machine reachable by other peers (typically a small VM with a public IP or DNS name).

### What `--listen` does

When the CLI is invoked with `--listen <addr>`:

1. It does everything a normal peer does (binds the directory, watches for changes, keeps the Automerge doc in sync, persists state to `.agentsync/`).
2. It additionally opens a websocket server on the given address.
3. Incoming connections are authenticated, then bridged to per-peer Automerge `SyncState` instances.
4. Sync messages flow bidirectionally; received changes are merged into the local doc and fanned out to other connected peers.

The rendezvous peer is otherwise identical to any other peer. Files written in its working directory are part of the synced vault.

### Wire protocol

WebSocket messages, MessagePack-encoded:

- `HELLO` — `{ vault_id, auth_token, op: "join" | "create" }`.
- `SYNC` — opaque Automerge sync message bytes, bidirectional. Automerge's protocol handles state vectors, incremental sync, and catch-up internally.
- `BLOB_FETCH` — `{ hash: String }` request a blob.
- `BLOB_PUSH` — `{ hash: String, bytes: Bytes }` send a blob.
- `PING` / `PONG` — keepalive.
- `ERROR` — error responses.

That's it. Six message types. Most of the sync intelligence is inside Automerge.

### Deployment

```bash
# On a small VM (e.g., $5/mo VPS):
$ agentsync init                              # generates vault_id and key
$ agentsync --listen 0.0.0.0:8443 \
    --tls-cert /etc/letsencrypt/.../fullchain.pem \
    --tls-key  /etc/letsencrypt/.../privkey.pem
```

For users who want easier setup, we provide:

- A single-binary install script (`curl | sh`).
- An optional Caddyfile snippet that handles TLS termination (recommended).
- A `systemd` unit file in the repo (not installed by default).

Backups are the user's responsibility: point any backup tool (restic, borgbackup, rclone) at `.agentsync/`.

---

## Core API (Rust)

The `agentsync-core` crate exposes the following public API. The CLI is a thin wrapper.

```rust
pub struct Vault {
    // private
}

impl Vault {
    pub async fn open(opts: OpenOptions) -> Result<Vault>;
    pub async fn create(opts: CreateOptions) -> Result<Vault>;
    
    pub fn id(&self) -> &VaultId;
    
    // Bound mode: the vault owns the directory
    pub async fn bind_directory(&mut self, path: &Path, opts: BindOptions) -> Result<Binding>;
    
    // Direct mode: caller manages I/O
    pub fn files(&self) -> Files<'_>;
    pub fn files_mut(&mut self) -> FilesMut<'_>;
    pub fn directories(&self) -> Directories<'_>;
    pub fn directories_mut(&mut self) -> DirectoriesMut<'_>;
    pub fn history(&self) -> History<'_>;
    
    // Connection management
    pub async fn connect(&mut self) -> Result<()>;
    pub async fn disconnect(&mut self) -> Result<()>;
    pub fn subscribe(&self) -> EventStream;  // Stream<Item = VaultEvent>
    
    // Listening (rendezvous mode)
    pub async fn listen(&mut self, addr: SocketAddr, tls: TlsConfig) -> Result<()>;
    
    pub async fn close(self) -> Result<()>;
}

pub struct OpenOptions {
    pub rendezvous_url: Option<String>,
    pub vault_id: VaultId,
    pub vault_key: [u8; 32],
    pub storage_path: PathBuf,  // typically ./.agentsync
}

pub struct CreateOptions {
    pub rendezvous_url: Option<String>,
    pub vault_key: Option<[u8; 32]>,  // generated if None
    pub storage_path: PathBuf,
}

pub struct BindOptions {
    pub exclude_patterns: Vec<String>,    // gitignore-style
    pub include_patterns: Vec<String>,    // optional allowlist
    pub attachment_max_bytes: u64,        // default 10 MB
    pub text_file_max_bytes: u64,         // default 1 MB
}

pub struct Files<'a> { /* ... */ }
impl Files<'_> {
    pub fn read(&self, path: &str) -> Result<String>;
    pub fn list(&self) -> Result<Vec<String>>;
    pub fn exists(&self, path: &str) -> bool;
    pub fn hash(&self, path: &str) -> Result<String>;
}

pub struct FilesMut<'a> { /* ... */ }
impl FilesMut<'_> {
    pub fn write(&mut self, path: &str, content: &str) -> Result<()>;
    pub fn delete(&mut self, path: &str) -> Result<()>;
    pub fn rename(&mut self, from: &str, to: &str) -> Result<()>;
}

pub struct Directories<'a> { /* ... */ }
impl Directories<'_> {
    pub fn list(&self, path: &str) -> Result<Vec<DirEntry>>;
}

pub struct DirectoriesMut<'a> { /* ... */ }
impl DirectoriesMut<'_> {
    pub fn create(&mut self, path: &str) -> Result<()>;
    pub fn delete(&mut self, path: &str, recursive: bool) -> Result<()>;
    pub fn rename(&mut self, from: &str, to: &str) -> Result<()>;
}

pub struct History<'a> { /* ... */ }
impl History<'_> {
    // Restore to any moment within the retention window
    pub async fn restore_to_time(&mut self, timestamp_ms: i64) -> Result<()>;
    pub async fn restore_to_heads(&mut self, heads: &[ChangeHash]) -> Result<()>;
    
    // Named labels
    pub fn create_label(&mut self, label: &str) -> Result<()>;
    pub fn list_labels(&self) -> Result<Vec<Label>>;
    pub async fn restore_label(&mut self, label: &str) -> Result<()>;
    pub fn delete_label(&mut self, label: &str) -> Result<()>;
    
    // Inspection
    pub fn change_log(&self, from_ms: i64, to_ms: i64) -> impl Iterator<Item = ChangeMeta>;
    pub fn diff(&self, from_heads: &[ChangeHash], to_heads: &[ChangeHash]) -> Result<DiffResult>;
}

pub enum VaultEvent {
    Connected,
    Disconnected,
    FileChanged { path: String },
    SyncProgress { percent: u8 },
    Error(String),
}
```

### FilesystemAdapter abstraction

```rust
#[async_trait]
pub trait FilesystemAdapter: Send + Sync {
    async fn read(&self, path: &Path) -> Result<Vec<u8>>;
    async fn write(&self, path: &Path, content: &[u8]) -> Result<()>;
    async fn delete(&self, path: &Path) -> Result<()>;
    async fn list(&self, path: &Path) -> Result<Vec<DirEntry>>;
    fn watch(&self, path: &Path) -> Result<Box<dyn Watcher>>;
    async fn hash(&self, path: &Path) -> Result<String>;
}
```

The CLI provides `NodeFsAdapter` — an implementation backed by `tokio::fs` and `notify`. The trait abstraction is here for v2: a future Obsidian plugin (compiled to WASM) would provide its own adapter backed by Obsidian's `DataAdapter`.

### Loop suppression and dirty-set draining

The `Binding` returned by `bind_directory` implements:

- A coalesced write queue: filesystem events for the same file within a 50ms window collapse into one.
- Content-acknowledged suppression: when the core writes a file because of an incoming Automerge change, it records the content hash in a short-TTL set. If the filesystem watcher fires for that file with matching content, it's ignored.
- Atomic writes: incoming updates are written to `${path}.agentsync-tmp` and renamed.

---

## CLI

Single binary, distributed via Homebrew, install-script, and direct download.

The dominant operation — running the sync engine on a directory — is the **default behavior** when invoked with no subcommand. Other operations are explicit subcommands.

```
agentsync [path] [--listen ADDR]
  Default: start syncing a directory. Foreground only.
  Equivalent to `agentsync watch [path]`.
  
  If [path] is omitted, defaults to the current directory.
  --listen ADDR also accepts incoming connections (acts as rendezvous).
  
  Errors out clearly if the directory has no .agentsync/config.toml.

agentsync watch [path] [--listen ADDR]
  Explicit form of the default.

agentsync init [--rendezvous URL] [--key-source ARG]
  Creates a new vault on the configured rendezvous peer. Prints vault_id
  and key, writes .agentsync/config.toml in the current directory.

agentsync clone <vault-id> <local-path> [--rendezvous URL] [--key KEY]
  Connect to an existing vault and materialize it locally.

agentsync status [path]
  Show connection state, last sync time, file count.

agentsync push [path]
  One-shot: scan directory, push any local changes, exit.

agentsync pull [path]
  One-shot: pull latest, write to disk, exit.

# Recovery
agentsync restore-at <timestamp> [--into PATH]
  Restore to a specific past moment within the retention window.

agentsync snapshot create <label>
  Mark the current heads with a human-readable label.

agentsync snapshot list
agentsync snapshot restore <label>
agentsync snapshot delete <label>

agentsync diff <heads-or-timestamp> [<heads-or-timestamp>]
  Show what changed between two points (or one point and now).

# Maintenance
agentsync compact [path]
  Run a compaction pass on this peer.

agentsync key generate
agentsync key store [--keyring NAME]
agentsync key show

agentsync help [<command>]
agentsync version
```

### Argument resolution rules

1. No args → run watch on the current directory.
2. First arg is a known subcommand (`init`, `clone`, `snapshot`, etc.) → run that subcommand.
3. First arg is a flag (starts with `-`) → run watch with those flags.
4. First arg is a path-like (existing directory or starts with `/`, `.`, `~`) → run watch on that path.
5. Otherwise → error: `Unknown command: <arg>. Run 'agentsync --help' for usage.`

### Configuration

Read in order: CLI flags → env vars (`AGENTSYNC_*`) → `.agentsync/config.toml` in the bound directory → `~/.config/agentsync/config.toml`.

```toml
[vault]
id = "01H..."
rendezvous_url = "wss://sync.example.com"

[key]
source = "keyring"  # or "env" or "file"
keyring_name = "agentsync-default"

[sync]
exclude = ["**/.git/**", "**/node_modules/**", "**/.DS_Store"]
include = ["**/*.md", "**/*.txt"]
attachment_max_bytes = 10485760
log_retention_days = 30
```

---

## Testing strategy — the E2E suite is the product

### Principle

E2E tests run the actual CLI binary in a multi-peer configuration inside Docker Compose. One container is the rendezvous (`agentsync --listen`), others are clients. Tests do not mock the network and do not mock the filesystem. If the test is green, the binary works.

Unit tests (`cargo test`) cover pure functions (path normalization, auth token derivation, schema operations on Automerge docs). Integration tests cover the core in isolation. **E2E tests gate releases.**

### Test environment

`docker-compose.test.yml` provides:

- One container running `agentsync --listen` (the rendezvous peer)
- N "client containers" each running the CLI binary, connecting to the rendezvous

There's no separate database service to provision, no object storage, no backend image. The whole test stack is just N+1 instances of the same CLI binary, each with its own working directory mounted as a volume.

The E2E test harness is itself a Rust crate under `tests/e2e/` that uses `testcontainers-rs` to manage the Docker stack and drives the CLIs via shell commands and assertions on filesystem state.

```rust
// Sketch of the E2E harness API
pub struct TestVault { /* ... */ }

impl TestVault {
    pub async fn new() -> Self;
    pub async fn add_client(&self, name: &str) -> TestClient;
    pub async fn destroy_client(&self, name: &str) -> Result<()>;
    pub async fn restart_rendezvous(&self) -> Result<()>;
}

pub struct TestClient {
    pub directory: PathBuf,  // host-side path mapped into the container
}

impl TestClient {
    pub async fn write_file(&self, path: &str, content: &str) -> Result<()>;
    pub async fn read_file(&self, path: &str) -> Result<String>;
    pub async fn delete_file(&self, path: &str) -> Result<()>;
    pub async fn wait_for_file(&self, path: &str, expected: &str, timeout: Duration) -> Result<()>;
    pub async fn wait_for_sync(&self, timeout: Duration) -> Result<()>;
    pub async fn disconnect(&self) -> Result<()>;
    pub async fn reconnect(&self) -> Result<()>;
}
```

### Required E2E tests

These are the tests that MUST exist and pass before v1 ships:

**Basic sync**
- `two_clients_one_write`
- `two_clients_concurrent_edit_different_files`
- `two_clients_concurrent_edit_same_file`: edits in different positions merge.
- `three_clients_fanout`
- `delete_propagates`
- `rename_propagates`: A renames a file, B sees the rename (file ID preserved).

**Directory operations**
- `empty_directory_syncs`
- `directory_rename_atomic`: A renames `/research` containing 50 files; B sees the rename as one atomic transition.
- `recursive_delete_atomic`
- `nested_create`: A creates `/a/b/c/file.md`; B materializes the full chain.

**Connection lifecycle**
- `offline_edits_merge_on_reconnect`
- `rendezvous_restart_survives`
- `client_crash_recovery`: kill mid-write, restart, state is consistent (Automerge doc.bin is valid).

**Auth**
- `wrong_key_cannot_connect`
- `tls_required`
- `tls_cert_validation`

**PITR and snapshots**
- `restore_to_timestamp`: 100 timestamped edits; restore to time of edit #50; content matches.
- `restore_is_additive`: restore to T; subsequent edits on other peers merge with restored state.
- `label_creates_and_restores`
- `label_syncs_across_peers`: A creates label, B can see and restore from it (labels live in the Automerge doc and sync automatically).
- `delete_and_recreate_same_path`: file deleted, new file created at same path; restore to a time between shows nothing; restore to a time during the original shows original content.

**Loop suppression**
- `no_echo_on_incoming_update`

**Compaction**
- `compaction_shrinks_doc`: 10,000 small edits, run compaction, doc.bin size drops.
- `compaction_preserves_content`
- `compaction_preserves_pitr_in_window`
- `compaction_prunes_out_of_window_labels`

**Edge cases**
- `binary_file_handling`: PNG syncs as a blob.
- `excluded_files_not_synced`
- `unicode_paths`
- `case_sensitivity`
- `large_vault_cold_start`: 10,000-file vault starts up in < 5 seconds (loads from doc.bin).

**CLI invocation**
- `bare_invocation_watches`
- `bare_invocation_no_config_errors`
- `bare_invocation_with_flags`
- `unknown_subcommand_errors`
- `bare_invocation_with_path`

---

## Performance evaluation suite

Performance is a core part of the product. We maintain a separate benchmark suite (`tests/perf/`) using `criterion` for proper statistical benchmarking, plus end-to-end perf tests that exercise the full binary.

### Principles

- Benchmarks run against the real CLI binary in a multi-peer configuration.
- Each benchmark records p50, p95, p99 latencies and outputs JSON results.
- Results are committed to `benchmarks/history/`.
- A PR that regresses any benchmark by more than 20% fails CI.

### Required benchmarks

**Latency benchmarks**:
- `bench_single_char_edit`: latency for a single-character edit to propagate A→B.
- `bench_file_create`
- `bench_file_delete`
- `bench_rename`
- `bench_directory_rename`: large directory rename propagation.

**Throughput benchmarks**:
- `bench_bulk_create`: 100, 1000, 10000 small files sync to a second peer.
- `bench_rapid_edits`: 1000 rapid edits to a single file converge.
- `bench_concurrent_writers`: 10 peers each writing 100 files converge.

**Resource benchmarks**:
- `bench_memory_1k_files`: peak resident memory, 1,000 markdown files.
- `bench_memory_10k_files`: peak resident memory, 10,000 markdown files.
- `bench_cold_start`: time from `agentsync` invocation to first successful sync.
- `bench_binary_size`: must be < 15 MB on Linux x86_64.

**PITR benchmarks**:
- `bench_restore_recent`: restore to a recent timestamp.
- `bench_restore_near_retention`: restore near the retention boundary.
- `bench_label_create`
- `bench_compaction_cost`: time to compact a doc with 10k changes.

**Automerge-specific micro-benchmarks** (using `criterion` directly on the core crate):
- `bench_apply_change`: throughput of applying single changes.
- `bench_save_load`: round-trip serialization cost.
- `bench_sync_message`: cost of generating/receiving a sync message at various doc sizes.

### Performance dashboard

Benchmark JSON output is rendered as an HTML dashboard committed to `docs/perf/`.

---

## Project structure

```
agentsync/
├── Cargo.toml                     # workspace manifest
├── Cargo.lock
├── crates/
│   ├── agentsync-core/            # the SDK as a library
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── vault.rs           # Vault struct
│   │       ├── binding.rs         # bind_directory implementation
│   │       ├── auth.rs            # Auth token derivation
│   │       ├── doc/
│   │       │   ├── mod.rs         # Automerge schema operations
│   │       │   ├── files.rs       # Files API
│   │       │   ├── directories.rs # Directories API
│   │       │   └── history.rs     # History API
│   │       ├── fs/
│   │       │   ├── mod.rs         # FilesystemAdapter trait
│   │       │   ├── node_adapter.rs # tokio::fs + notify backend
│   │       │   └── suppression.rs # Loop / dirty-set logic
│   │       ├── net/
│   │       │   ├── mod.rs
│   │       │   ├── client.rs      # Outbound websocket connection
│   │       │   ├── server.rs      # Inbound listener (--listen)
│   │       │   ├── fanout.rs      # Update fanout
│   │       │   └── protocol.rs    # Wire protocol types
│   │       ├── store/
│   │       │   ├── mod.rs
│   │       │   ├── doc_store.rs   # doc.bin persistence
│   │       │   ├── snapshots.rs   # snapshots/index.json
│   │       │   ├── blobs.rs       # Content-addressed blob storage
│   │       │   └── compaction.rs
│   │       └── error.rs
│   │
│   └── agentsync-cli/             # the CLI binary
│       ├── Cargo.toml
│       └── src/
│           ├── main.rs
│           ├── commands/
│           │   ├── init.rs
│           │   ├── watch.rs
│           │   ├── snapshot.rs
│           │   ├── restore.rs
│           │   └── ...
│           ├── config.rs
│           └── keyring.rs
│
├── tests/
│   ├── e2e/                       # E2E test crate
│   │   ├── Cargo.toml
│   │   ├── src/
│   │   │   └── harness.rs
│   │   └── tests/
│   │       ├── basic_sync.rs
│   │       ├── directory_ops.rs
│   │       ├── auth.rs
│   │       ├── pitr.rs
│   │       ├── snapshots.rs
│   │       ├── compaction.rs
│   │       └── cli.rs
│   └── perf/                      # Perf benchmarks
│       ├── Cargo.toml
│       └── benches/
│           ├── latency.rs
│           ├── throughput.rs
│           ├── memory.rs
│           ├── pitr.rs
│           └── automerge_micro.rs
│
├── benchmarks/
│   └── history/                   # JSON results, committed per release
│
├── docs/
│   ├── perf/                      # Auto-generated perf dashboard
│   ├── architecture.md
│   └── deployment.md
│
├── docker-compose.test.yml
├── docker-compose.dev.yml
├── .github/workflows/
│   ├── unit.yml
│   ├── e2e.yml
│   └── perf.yml
└── README.md
```

Cargo workspace with two member crates: `agentsync-core` (the library) and `agentsync-cli` (the binary). E2E tests and perf benchmarks live under `tests/` as separate crates.

Build outputs:
- `agentsync` binary → published to GitHub Releases AND Homebrew formula AND `cargo install agentsync-cli`.
- `agentsync-core` → published to crates.io.

A future TypeScript/WASM wrapper (v2) would compile `agentsync-core` to WASM with `wasm-bindgen` and publish as `@agentsync/sdk` on npm.

---

## Tech stack

- **Language:** Rust, edition 2024.
- **CRDT:** `automerge` (latest 3.x) — core data structure and sync protocol.
- **Async runtime:** `tokio` with `rt-multi-thread`.
- **Filesystem watching:** `notify` (cross-platform fsevents/inotify/ReadDirectoryChangesW).
- **Websocket server + client:** `tokio-tungstenite` for the websocket layer; `axum` for the listener if we need any HTTP endpoints (for v1 we don't, but having it available is cheap).
- **TLS:** `rustls` with `tokio-rustls` (no OpenSSL dependency, pure Rust).
- **Wire codec:** `rmp-serde` (MessagePack via serde).
- **CLI parsing:** `clap` v4 with `derive`.
- **Config:** `toml` (parsing) + `serde` (deserializing into structs).
- **Logging:** `tracing` + `tracing-subscriber`.
- **Crypto for auth tokens:** `hmac` + `sha2` (HMAC-SHA256, RustCrypto).
- **Random key generation:** `rand` with `OsRng`.
- **Hashing for blob content addressing:** `sha2`.
- **Error handling:** `thiserror` for library errors, `anyhow` in the CLI.
- **Testing:**
  - Unit/integration: `cargo test`.
  - E2E: `tokio::test` + `testcontainers` to manage Docker.
  - Benchmarks: `criterion` for micro-benchmarks; custom Rust runners for full-binary perf tests.

Notably absent: no database driver, no S3 SDK, no third-party crypto beyond standard primitives.

### Why Rust over TypeScript

- **Smaller binary, faster startup.** Target < 15 MB statically linked, < 50 ms cold start. Critical for a tool agents invoke frequently.
- **Lower memory footprint.** Important for the rendezvous peer holding many concurrent connections and a full Automerge doc.
- **No GC stalls.** Important under high-frequency edit loads.
- **Native Automerge performance.** Automerge's canonical implementation is Rust. We get full speed and full API surface, not a WASM wrapper.
- **WASM target for v2.** When we add the TypeScript SDK and Obsidian plugin, the same `agentsync-core` compiles to WASM — no rewrite required.
- **Better cross-platform distribution story.** `cargo build`, `cross`, and a single static binary work everywhere. No Node/Bun dependency.

The cost is roughly +1-2 weeks of v1 development time vs TypeScript, mostly in initial setup and the borrow-checker learning curve. The long-term return is a foundation that scales to every platform we'd want to target.

---

## Milestones

### M0 — Skeleton (week 1)
- Cargo workspace, lint (clippy), format (rustfmt), CI.
- `agentsync-core` and `agentsync-cli` crates that build and produce a binary.
- Docker-based E2E harness can spin up an N+1 peer test stack.
- Trivial E2E test passes (CLI prints version).

### M1 — Single-peer roundtrip (weeks 2-3)
- Automerge document schema (files + directories + labels + meta).
- `doc.bin` save/load with atomic-rename persistence.
- Filesystem adapter (tokio::fs + notify) with binding.
- Auth token derivation and validation.
- Single peer can `init`, `write`, restart, and recover state from `doc.bin`.
- E2E tests: client_crash_recovery passes; CLI invocation tests pass.

### M2 — Two-peer sync via rendezvous (weeks 4-5)
- Websocket server (`--listen`) with TLS support via rustls.
- Websocket client and connection management.
- Bridge from websocket frames to Automerge sync messages and back.
- Loop suppression / dirty-set draining.
- Multi-peer fanout from the rendezvous.
- E2E tests: all "Basic sync", "Directory operations", "Connection lifecycle", "Auth" tests pass.

### M3 — PITR and snapshots (week 6)
- `restore-at <timestamp>` and `restore-to-heads` implementations.
- Named labels in Automerge doc + cached `snapshots/index.json`.
- `agentsync diff` command.
- E2E tests: all "PITR and snapshots" tests pass.

### M4 — Compaction, blobs, hardening (week 7)
- Automerge-based compaction with retention window.
- Binary file / attachment handling via content-addressed blobs.
- Performance benchmarks in CI with regression detection.
- E2E tests: all remaining tests pass; perf benchmarks all green.

### M5 — Distribution (week 8)
- Static binary builds via `cross` for macOS (x64+arm64), Linux (x64+arm64), Windows (x64).
- Homebrew formula.
- `cargo install agentsync-cli` works.
- README with quickstart, deployment guide, agent integration recipes.
- Public release.

Total: 8 weeks. Slightly longer than the TypeScript estimate due to Rust's higher initial setup cost, but balanced by Automerge giving us PITR and sync for free instead of building them.

---

## Finalized design decisions

1. **Rust + Automerge.** Rust core in `agentsync-core`; CLI binary `agentsync` wraps it. Automerge for the CRDT — its native history and JSON-shaped data model are a much better fit than Yjs for this use case. TypeScript/WASM wrapper deferred to v2.

2. **One Automerge document per vault.** Tree, file metadata, file contents, and labels all live in one doc. PITR, snapshots, and sync all operate on this single doc. Per-file partitioning is a v2 scale lever.

3. **The Automerge document is the source of truth.** No separate append log — the document already contains its full history as a DAG. `doc.bin` is the only state file. Snapshots are labels (heads) into that history.

4. **No periodic snapshot scheduler.** The Automerge document already contains the full history. Users (or agents) create labels at meaningful points.

5. **PITR is a primary feature.** `restore_to_time(ms)` and `restore_to_heads(...)` use Automerge's `*_at(heads)` primitives. Restore is additive (forward-only changes), not destructive.

6. **No app-layer encryption.** TLS 1.3 between peers, vault key as auth, full-disk encryption (user's responsibility) for at-rest. E2EE with relay-only rendezvous is reserved for v2 hosted offering.

7. **CLI runs in foreground only.** Backgrounding via shell.

8. **Bare `agentsync` is a shortcut for `agentsync watch`.** Dominant operation is the default.

9. **One vault per process; multiple processes for multiple vaults.**

10. **No backend service.** "Server" is `agentsync --listen` running on a VM.

11. **No users in v1.** Vaults are namespace primitives, accessed by key.

---

## Success criteria for v1

The product ships when:

1. Every E2E test in the required list passes on every PR.
2. Every performance benchmark passes its budget on every PR.
3. The CLI binary is < 15 MB and starts in < 50 ms cold (with warm cache; < 5 s for 10k-file vault cold load).
4. The README's quickstart works in under 5 minutes for a new user.

When all four are true, cut v1.0 and ship.
