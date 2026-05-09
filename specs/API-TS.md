# API-TS.md — TypeScript SDK Public API

> Normative for the published `@agentsync/sdk` API. See [SPEC.md §
> Conformance language](./SPEC.md#conformance-language).

This document specifies the public API of the TypeScript SDK at
`sdks/typescript/`, published as `@agentsync/sdk`. It covers the
high-level `Vault` class, the wasm boundary it sits on, and the
JavaScript-side adapter interfaces a consumer can implement (storage,
transport).

For byte-level semantics underlying these methods, see the linked
specs.

---

## 1. Package layout

The SDK has two entrypoints that target different runtimes:

| Entrypoint | File | Wasm target | Built-in adapters |
|---|---|---|---|
| `@agentsync/sdk`         | `src/index.ts` | `nodejs`  | `MemoryStorage`, `NodeFsStorage`, `nodeWsTransport` |
| `@agentsync/sdk/web`     | `src/web.ts`   | `bundler` | `OpfsStorage` |

A consumer **MUST** import from the entrypoint matching the target
runtime. Importing the wrong one will load a wasm module the runtime
cannot execute.

The two entrypoints export the same `Vault`, `Doc`, `Identity`,
`Pubkey`, `SyncState`, frame codec, and type definitions. They differ
only in the bundled adapters.

---

## 2. Top-level exports (both entrypoints)

```ts
// Wasm-backed primitives (re-exported):
export { Identity, Pubkey, Doc, SyncState };

export function parseAuthorizedKeys(body: string): AuthorizedPeer[];
export function renderAuthorizedKeys(entries: AuthorizedPeer[]): string;

export function randomNonce(): Uint8Array;
export function buildTranscript(
  hubNonce:            Uint8Array,
  peerNonce:           Uint8Array,
  tlsCertFingerprint:  Uint8Array,
  hubPubkey:           Uint8Array,
  peerPubkey:          Uint8Array,
): Uint8Array;

export function encodeFrame(frame: Frame): Uint8Array;
export function decodeFrame(bytes: Uint8Array): Frame;

export function contentHash(bytes: Uint8Array): string; // lowercase hex SHA-256
export function schemaVersion(): number;                // == 1
export function defaultPort(): number;                  // == 443
export function normalizeRendezvousUrl(url: string): string;

// High-level Vault:
export const Vault: {
  create(opts: CreateOptions): Promise<VaultInstance>;
  open  (opts: OpenOptions):   Promise<VaultInstance>;
};

// Type re-exports:
export type {
  AuthorizedPeer, FileMeta, DirectoryMeta, Label,
  Frame, FrameTag, HelloOp,
  StorageAdapter, TransportAdapter, TransportConn,
  VaultEvent, VaultOptions, ReconnectOptions,
  CreateOptions, OpenOptions, VaultInstance,
};
```

These exports correspond 1-to-1 with items in [API-RUST.md](./API-RUST.md)
where applicable. The wire-related helpers (`encodeFrame`,
`buildTranscript`, etc.) cross the wasm boundary; their byte-level
behavior is fully specified by [WIRE.md](./WIRE.md).

### 2.1 Adapters bundled with `@agentsync/sdk` (Node/Bun)

```ts
export class MemoryStorage implements StorageAdapter {}
export const memoryStorage: () => MemoryStorage;

export class NodeFsStorage implements StorageAdapter {}
export const nodeFsStorage: (dir: string) => NodeFsStorage;

export function nodeWsTransport(): TransportAdapter;
```

### 2.2 Adapters bundled with `@agentsync/sdk/web`

```ts
export class OpfsStorage implements StorageAdapter {}
export const opfsStorage: () => OpfsStorage;
```

The web entrypoint does **not** ship a built-in transport; the
browser's global `WebSocket` is used directly.

---

## 3. The `Vault` class

The `Vault` class is the high-level entry point. Unlike the Rust
`Vault`, the TypeScript SDK implements the connection state machine in
JavaScript on top of wasm primitives — the wasm crate currently does
not expose a top-level `Vault` (see [HOST.md § 7.3](./HOST.md#73-current-state-of-the-reference)).

### 3.1 Construction

```ts
export interface VaultOptions {
  storage:        StorageAdapter;
  identity?:      Identity;
  vaultId?:       string;
  rendezvousUrl?: string;
  hubPubkey?:     Uint8Array;        // 32 bytes
  name?:          string;
  transport?:     TransportAdapter;  // default: WebSocket
}

export interface CreateOptions extends VaultOptions {}
export interface OpenOptions   extends VaultOptions {}

export class Vault {
  static async create(opts: CreateOptions): Promise<Vault>;
  static async open  (opts: OpenOptions):   Promise<Vault>;
}
```

`storage` is required for both `create` and `open` — the SDK has no
default storage. In the browser, pass `opfsStorage()`. In Node, pass
`nodeFsStorage(path)` or `memoryStorage()`.

`identity` may be passed explicitly. If omitted:

- For `create`: the SDK generates a fresh keypair and persists its seed
  via `storage.saveIdentitySeed`.
- For `open`: the SDK loads the seed via `storage.loadIdentitySeed`. If
  no seed exists, the call **MUST** error.

`hubPubkey`, when set, pins the hub identity (TOFU). On a handshake
mismatch the connection **MUST** fail. See [AUTH.md § Hub trust](./AUTH.md#hub-trust).

`transport`, when set, replaces the default. See § 6.

### 3.2 Accessors

```ts
class Vault {
  vaultIdValue(): string;
  identityRef(): Identity;
  isConnected(): boolean;
}
```

### 3.3 File operations

```ts
class Vault {
  writeTextFile(path: string, content: string): Promise<string>;
  readTextFile (path: string):                  Promise<string>;
  fileExists   (path: string):                  boolean;
  deleteFile   (path: string):                  Promise<void>;
  renameFile   (from: string, to: string):      Promise<void>;
  listFiles    ():                              FileMeta[];
}
```

`writeTextFile` returns the file's UUID (as a string). Path
normalization rules from [DOCUMENT.md § 5](./DOCUMENT.md#path-normalization)
apply.

`fileExists` and `listFiles` are synchronous because they read in-memory
document state; the others are async because they may trigger
persistence.

### 3.4 Directory operations

```ts
class Vault {
  createDirectory(path: string):                                Promise<string>;
  deleteDirectory(path: string, recursive?: boolean):           Promise<void>;
  listDirectories():                                            DirectoryMeta[];
}
```

`recursive` defaults to `false`. Setting it `true` performs the
atomic recursive delete specified in
[DOCUMENT.md § 3.3](./DOCUMENT.md#recursive-delete).

### 3.5 Labels and history

```ts
class Vault {
  createLabel  (name: string):                Promise<void>;
  deleteLabel  (name: string):                Promise<void>;
  listLabels   ():                            Label[];
  restoreToLabel(name: string):               Promise<void>;
  restoreToTime(targetMs: number):            Promise<void>;
}
```

Label semantics match the Rust API. See
[API-RUST.md § 3.5](./API-RUST.md#35-history-and-labels).

### 3.6 Connection management

```ts
class Vault {
  connect():               Promise<void>;
  connectWithReconnect(opts?: ReconnectOptions): Promise<void>;
  disconnect():            Promise<void>;
}

export interface ReconnectOptions {
  maxAttempts?:     number;
  initialBackoffMs?: number;
  maxBackoffMs?:    number;
}
```

`connect` performs the four-message handshake (see
[WIRE.md § 4](./WIRE.md#handshake-normative)). On `Auth` failure the
promise **MUST** reject with an error whose message identifies the
failure; the connection **MUST NOT** be silently retried by `connect`
itself.

`connectWithReconnect` retries on transport errors with exponential
backoff between `initialBackoffMs` (default 250) and `maxBackoffMs`
(default 30 000). It **MUST** stop on auth failure (the credentials
are wrong) and on the supplied `AbortSignal` if any.

### 3.7 Lifecycle

```ts
class Vault {
  close(): Promise<void>;
}
```

`close` flushes pending writes through `storage`, disconnects, and
releases the underlying wasm `Doc`. After `close`, all other methods
on the instance **MUST** throw.

---

## 4. Wasm-backed primitives

These are re-exported from `agentsync-wasm`. Their signatures match
the wasm-bindgen output.

### 4.1 `Identity`

```ts
class Identity {
  static generate(): Identity;
  static fromSeed(seed: Uint8Array): Identity;
  seed(): Uint8Array;                         // 32 bytes
  pubkey(): Pubkey;
  sign(message: Uint8Array): Promise<Uint8Array>; // 64 bytes
  free(): void;
}
```

Note: `sign` is async to mirror the Rust `Signer` trait, even though
file-backed identities sign synchronously. A consumer **SHOULD**
always `await` it.

### 4.2 `Pubkey`

```ts
class Pubkey {
  static fromBytes(bytes: Uint8Array): Pubkey;       // 32 bytes
  static fromSshString(s: string): Pubkey;
  toSshString(): string;                              // "ssh-ed25519 <base64>"
  fingerprint(): string;                              // hex SHA-256
  bytes(): Uint8Array;
  verify(message: Uint8Array, signature: Uint8Array): boolean;
  free(): void;
}
```

### 4.3 `Doc`

```ts
class Doc {
  constructor(vaultId: string);
  static load(bytes: Uint8Array): Doc;
  save(): Uint8Array;
  saveIncremental(): Uint8Array;
  vaultId(): string;
  heads(): Uint8Array[];                              // each is 32 bytes

  merge(other: Doc): boolean;
  generateSyncMessage(state: SyncState): Uint8Array | undefined;
  receiveSyncMessage (state: SyncState, bytes: Uint8Array): boolean;

  // file ops (path is POSIX-normalized; see DOCUMENT.md)
  writeTextFile(path: string, content: string): string;  // returns FileId
  readFile     (path: string):                  string;
  fileExists   (path: string):                  boolean;
  deleteFile   (path: string):                  void;
  renameFile   (from: string, to: string):      void;
  writeAttachment(path: string, hash: string, size: number): string;
  listFiles():                                  FileMeta[];

  createDirectory(path: string):                string;
  deleteDirectory(path: string, recursive: boolean): void;
  listDirectories():                            DirectoryMeta[];

  createLabel  (name: string):                  void;
  deleteLabel  (name: string):                  void;
  listLabels   ():                              Label[];
  restoreToLabel(name: string):                 void;
  restoreToTime(targetMs: number):              void;

  free(): void;
}
```

`Doc` is a wasm-bindgen-managed object. Each method call crosses the
wasm boundary. Consumers **MUST** call `free()` when done with a `Doc`
unless they're using the high-level `Vault` (which manages its own
`Doc` lifetime).

### 4.4 `SyncState`

```ts
class SyncState {
  constructor();
  static decode(bytes: Uint8Array): SyncState;
  encode(): Uint8Array;
  free(): void;
}
```

A `SyncState` is one peer's view of the Automerge sync protocol state
machine. The high-level `Vault` maintains one per active peer; a
consumer using `Doc` directly is responsible for managing them.

---

## 5. Events

```ts
export type VaultEvent =
  | { kind: 'connecting';     url: string }
  | { kind: 'connected';      hub_pubkey: Uint8Array; vault_id: string }
  | { kind: 'disconnected';   reason: string }
  | { kind: 'sync-progress';  outbound: boolean }
  | { kind: 'doc-changed';    heads: Uint8Array[] }
  | { kind: 'error';          message: string };

class Vault {
  subscribe(listener: (e: VaultEvent) => void): () => void;
  events(): AsyncIterableIterator<VaultEvent>;
}
```

`subscribe` returns an unsubscribe function. `events()` is the
`AsyncIterableIterator` form, suitable for `for await ... of`.

The TypeScript event shape is **richer** than the Rust crate's
`VaultEventKind` enum (see [API-RUST.md § 3.7](./API-RUST.md#37-event-stream)).
This is intentional: the SDK orchestrates the connection state machine
in JS and has more state available to surface.

---

## 6. Adapter interfaces

A reimplementation of the SDK targeting a new runtime implements these
interfaces.

### 6.1 `StorageAdapter`

```ts
export interface StorageAdapter {
  loadDoc():                                   Promise<Uint8Array | null>;
  saveDoc(bytes: Uint8Array):                  Promise<void>;
  loadSyncState(peerKey: string):              Promise<Uint8Array | null>;
  saveSyncState(peerKey: string, bytes: Uint8Array): Promise<void>;
  loadIdentitySeed():                          Promise<Uint8Array | null>;
  saveIdentitySeed(seed: Uint8Array):          Promise<void>;
  loadSnapshots():                             Promise<Uint8Array | null>;
  saveSnapshots(bytes: Uint8Array):            Promise<void>;
  close():                                     Promise<void>;
}
```

Contract:

- `loadDoc` returns the saved `doc.bin` bytes, or `null` if no save has
  ever occurred.
- `saveDoc(bytes)` replaces the saved document atomically.
- `loadSyncState(peerKey)` / `saveSyncState(peerKey, bytes)` persist
  the per-peer Automerge sync state. `peerKey` is an opaque
  implementation-chosen string (typically the hex pubkey). The adapter
  **MUST NOT** parse `peerKey`.
- `loadIdentitySeed` / `saveIdentitySeed` persist the local
  ed25519 seed (32 bytes). On Node, `nodeFsStorage` uses the format
  defined in [STORAGE.md § 8.2](./STORAGE.md#private-file-format) so
  the same identity can be used by the Rust binary.
- `loadSnapshots` / `saveSnapshots` persist `snapshots/index.json`
  bytes — see [STORAGE.md § 3](./STORAGE.md#snapshotsindexjson).
- `close` releases any underlying handles (file locks, IndexedDB
  handles, etc.).

All saves **MUST** be atomic in the all-or-nothing sense (see
[STORAGE.md § 9](./STORAGE.md#atomic-write-summary)).

### 6.2 `TransportAdapter`

```ts
export interface TransportAdapter {
  connect(url: string, opts?: TransportConnectOpts): Promise<TransportConn>;
}

export interface TransportConnectOpts {
  pinnedCertFingerprint?: Uint8Array;  // 32 bytes
}

export interface TransportConn {
  send(bytes: Uint8Array):  Promise<void>;
  recv():                   AsyncIterable<Uint8Array>;
  channelBinding():         Uint8Array | null;
  close():                  Promise<void>;
}
```

This mirrors the Rust `Transport` / `Conn` traits in
[HOST.md § 4](./HOST.md#transport-transport-conn-listener-acceptor).

`channelBinding()` returns the SHA-256 of the hub's TLS cert DER if
the transport can recover it, else `null`. Returning `null` puts the
connection in degraded channel-binding mode. Browser `WebSocket`
**MUST** return `null` (the API does not expose the cert).

`pinnedCertFingerprint` is informational. The transport **MAY** use
it to short-circuit a known mismatch before the handshake; the engine
also enforces it independently.

### 6.3 Built-in adapters

`MemoryStorage` keeps everything in JS-side `Map`s. Useful for tests
and ephemeral browser tabs.

`NodeFsStorage(dir)` writes to a real `.agentsync/` directory at `dir`,
matching the layout in [STORAGE.md](./STORAGE.md). A `Vault` opened
with `NodeFsStorage(d)` is interoperable with a Rust binary opened on
the same `d`.

`OpfsStorage()` writes to the browser's Origin Private File System,
keyed under a fixed root chosen by the SDK. Format details:
implementation-defined (no Rust reader is expected to load OPFS).

`nodeWsTransport()` returns a `TransportAdapter` backed by the Node
`ws` package. The browser's global `WebSocket` is used as the implicit
default in `web.ts`.

---

## 7. Type definitions

```ts
export interface AuthorizedPeer {
  pubkey: string;   // "ssh-ed25519 AAAA..."
  label:  string;
}

export interface FileMeta {
  id:           string;
  path:         string;
  kind:         'Text' | 'Attachment';
  size:         number;
  created_at:   number;
  updated_at:   number;
  deleted_at?:  number | null;
  binary_hash?: string | null;
}

export interface DirectoryMeta {
  id:          string;
  path:        string;
  created_at:  number;
  deleted_at?: number | null;
}

export interface Label {
  name:          string;
  heads_b64:     string;     // base64 no-pad of N*32 bytes
  created_at_ms: number;
}

export type FrameTag =
  | 'hello_hub' | 'hello_peer' | 'proof_hub' | 'proof_peer'
  | 'sync' | 'blob_fetch' | 'blob_push'
  | 'ping' | 'pong' | 'error';

export type HelloOp = 'join' | 'create';

export type Frame =
  | { t: 'hello_hub';  vault_id: string;
      hub_identity_pubkey: Uint8Array; hub_nonce: Uint8Array;
      tls_cert_fingerprint: Uint8Array; vault_name?: string | null }
  | { t: 'hello_peer'; peer_identity_pubkey: Uint8Array;
      peer_nonce: Uint8Array; op: HelloOp }
  | { t: 'proof_hub';  sig: Uint8Array }
  | { t: 'proof_peer'; sig: Uint8Array }
  | { t: 'sync';       bytes: Uint8Array }
  | { t: 'blob_fetch'; hash: string }
  | { t: 'blob_push';  hash: string; bytes: Uint8Array }
  | { t: 'ping';       ts: number }
  | { t: 'pong';       ts: number }
  | { t: 'error';      message: string };
```

Type-level note: `FileMeta.kind` is the capitalized form
(`'Text'` / `'Attachment'`) at the TS API surface, even though the
Automerge document stores it as the lowercase `"text"` / `"attachment"`
string. The wasm boundary maps between the two. A reimplementation
**MAY** expose either form on its API surface, but **MUST** persist the
lowercase form in the document.

---

## 8. Error semantics

The SDK propagates errors as plain `Error` instances with a string
message. There is no exception class hierarchy in v1.

A reimplementation **SHOULD** expose error messages with stable
prefixes so consumers can pattern-match (e.g., `"auth: ..."`, `"protocol: ..."`).
The reference does not currently formalize this.

---

## 9. Symmetry with the Rust API

The TypeScript `Vault` is **not** a strict mirror of the Rust `Vault`:

| Capability | Rust | TS |
|---|---|---|
| `connect`, `disconnect`, `connectWithReconnect` | yes | yes |
| `listen` (hub mode) | yes | **no** |
| `bind_directory` (filesystem watch) | yes | **no** |
| `materialize` | yes | **no** |
| `peer_count`, `authorized_pubkeys` accessors | yes | **no** |
| Adapter-pluggable storage | **no** (filesystem only) | yes |
| Adapter-pluggable transport | **no** | yes |
| Async iterator events (`events()`) | **no** (broadcast::Receiver) | yes |

A reimplementation in another language **MAY** include or omit
features per its target environment but **MUST** document the gaps.

---

## 10. Cross-references

- [API-RUST.md](./API-RUST.md) — the Rust surface this layers on top
  of (via `agentsync-wasm`).
- [HOST.md](./HOST.md) — the trait factoring the wasm `Host` is
  expected to conform to in a future iteration.
- [WIRE.md](./WIRE.md) — protocol used by `Vault.connect`.
- [DOCUMENT.md](./DOCUMENT.md) — schema operated on by `Doc`.
- [STORAGE.md](./STORAGE.md) — formats `StorageAdapter` persists.
