# @agentsync/sdk

TypeScript / WebAssembly SDK for [agentsync](https://github.com/cjroth/agentsync).
Wraps the same Rust engine that powers the `agentsync` CLI, compiled to wasm32 and shipped with idiomatic TS bindings.

The SDK exposes a high-level `Vault` API (connect, sync, watch, restore, labels, file ops) **plus** the low-level CRDT / identity / frame primitives, so you can build everything from a one-liner Obsidian plugin to a custom Tauri app on the same engine.

## Install

```bash
npm install @agentsync/sdk
# or
bun add @agentsync/sdk
```

## High-level Vault API

```ts
import {
  Vault,
  Identity,
  memoryStorage,
  nodeFsStorage,
  nodeWsTransport,
} from '@agentsync/sdk';
import WebSocket from 'ws';

// Create a fresh local-only vault (no rendezvous yet).
const vault = await Vault.create({
  storage: nodeFsStorage('./my-vault/.agentsync'),
});

await vault.writeTextFile('notes/hello.md', '# hi\n');
const text = await vault.readTextFile('notes/hello.md');
console.log(vault.listFiles().map((f) => f.path));

// Snapshots & restore
await vault.createLabel('before-cleanup');
await vault.writeTextFile('notes/hello.md', 'oops');
await vault.restoreToLabel('before-cleanup');                // back to "# hi\n"
await vault.restoreToTime(Date.now() - 60_000);              // 1 minute ago

// Sync against an existing remote vault hosted by `agentsync --listen`.
const peer = await Vault.create({
  storage: nodeFsStorage('./peer/.agentsync'),
  vaultId: '6f1f1aa9-...',                                    // from the hub's `agentsync init`
  rendezvousUrl: 'wss://hub.example.com:443',
  transport: nodeWsTransport(WebSocket),
});

// Subscribe to events (also available as `for await of peer.events()`).
peer.subscribe((e) => console.log(e.kind, e));

// Connect once and run the sync loop:
peer.connect();
// ...or with auto-reconnect + exponential backoff:
peer.connectWithReconnect({ maxAttempts: Infinity, initialBackoffMs: 500 });

await peer.disconnect();
await peer.close();
```

The Vault class:
- Owns the protocol state machine (4-message handshake, Automerge incremental sync, channel-binding fingerprint check, reconnect supervisor).
- Persists `doc.bin`, the identity seed, and per-peer `SyncState` through whatever `StorageAdapter` you supply.
- Mirrors the Rust `Vault` API one-for-one: every Rust method has a camelCase TS twin.

### Vault methods

| Method | What it does |
| --- | --- |
| `Vault.create({ storage, identity?, vaultId?, rendezvousUrl?, hubPubkey?, name?, transport? })` | Initialize a new vault. Pass `vaultId` to join an existing remote vault. |
| `Vault.open({ storage, identity?, ... })` | Reopen a vault previously persisted to `storage`. |
| `vault.writeTextFile(path, content)` | Write or update a UTF-8 file. Returns the stable file id. |
| `vault.readTextFile(path)` | Read a file. |
| `vault.deleteFile(path)`, `vault.renameFile(from, to)` | Mutations. |
| `vault.listFiles()`, `vault.fileExists(path)` | Read-only queries. |
| `vault.createDirectory(path)`, `vault.deleteDirectory(path, recursive?)`, `vault.listDirectories()` | Directory ops. |
| `vault.createLabel(name)`, `vault.deleteLabel(name)`, `vault.listLabels()` | Snapshots. |
| `vault.restoreToLabel(name)`, `vault.restoreToTime(unixMs)` | Additive history rewind. |
| `vault.connect()` | Open one rendezvous session, run the sync loop, return when it closes. |
| `vault.connectWithReconnect(opts?)` | Same with exponential backoff. |
| `vault.disconnect()` | Drop the active session and the reconnect supervisor. |
| `vault.subscribe((event) => …)` / `vault.events()` | Vault event stream — `connecting`, `connected`, `disconnected`, `doc-changed`, `sync-progress`, `error`. |
| `vault.isConnected()`, `vault.vaultIdValue()`, `vault.identityRef()` | Accessors. |
| `vault.close()` | Persist, drop the connection, free wasm memory. |

### Adapters bundled with the SDK

| Adapter | Use when |
| --- | --- |
| `memoryStorage()` (`MemoryStorage`) | Tests, ephemeral browser sessions. |
| `nodeFsStorage(rootDir)` (`NodeFsStorage`) | Node, Bun, Electron main process, VS Code extensions. Atomic write-tmp-then-rename. |
| `opfsStorage(rootName?)` (`OpfsStorage`) | Browser apps. Uses the Origin Private File System with `FileSystemSyncAccessHandle` from a Web Worker for fast writes; falls back to async writable streams on the main thread. |
| `nodeWsTransport(ws)` | Node WebSocket transport. Pass the [`ws`](https://www.npmjs.com/package/ws) constructor. Exposes the peer TLS cert SHA-256 via `channelBinding()` for end-to-end channel binding. |
| Browser default transport | `globalThis.WebSocket` is used automatically when no `transport` is supplied. Browsers don't expose peer certs, so channel binding falls back to the application-layer signature only. |

You can supply your own `StorageAdapter` / `TransportAdapter` for unusual hosts (Tauri-backed Rust filesystem, Cloudflare Durable Objects, an Obsidian vault adapter — anything that implements the trait).

## Low-level primitives

Everything the high-level Vault is built from is also exported, in case you want
to build something Vault doesn't cover:

```ts
import {
  Identity,
  Pubkey,
  Doc,
  SyncState,
  parseAuthorizedKeys,
  renderAuthorizedKeys,
  randomNonce,
  buildTranscript,
  encodeFrame,
  decodeFrame,
  contentHash,
  schemaVersion,
  defaultPort,
  normalizeRendezvousUrl,
} from '@agentsync/sdk';

// Two Doc instances + their SyncStates can converge end-to-end without any
// network — the same primitives Vault uses internally:
const a = new Doc('vault-1');
const b = new Doc('vault-1');
a.writeTextFile('a.md', 'from A');
b.writeTextFile('b.md', 'from B');

const aState = new SyncState();
const bState = new SyncState();
for (let i = 0; i < 50; i++) {
  const m1 = a.generateSyncMessage(aState);
  if (m1) b.receiveSyncMessage(bState, m1);
  const m2 = b.generateSyncMessage(bState);
  if (m2) a.receiveSyncMessage(aState, m2);
  if (!m1 && !m2) break;
}
// Both docs now have a.md and b.md.
```

| Primitive | Notes |
| --- | --- |
| `Identity` | `generate`, `fromSeed`, `seed`, `sign`, `pubkey`. ssh-agent backend is native-only — wasm uses file-backed identities. |
| `Pubkey` | `fromBytes`, `fromSshString`, `toSshString`, `fingerprint`, `bytes`, `verify`. |
| `Doc` | Automerge-backed vault doc. CRUD on files & directories, labels, `heads`, `merge`, `save` / `load` / `saveIncremental`, `generateSyncMessage` / `receiveSyncMessage`, `restoreToLabel`, `restoreToTime`. |
| `SyncState` | Per-peer Automerge sync state. `encode` / `decode` for persistence so reconnects don't replay history. |
| `parseAuthorizedKeys` / `renderAuthorizedKeys` | SSH-style auth file. |
| `encodeFrame` / `decodeFrame` | msgpack codec for the wire protocol. |
| `buildTranscript` / `randomNonce` | Handshake helpers — match the bytes the Rust hub signs. |
| `contentHash` | SHA-256 hex (matches the on-disk content hash format). |
| `schemaVersion`, `defaultPort`, `normalizeRendezvousUrl` | Constants & helpers. |

## Entry points

| Import | Target | Use when |
| --- | --- | --- |
| `@agentsync/sdk` | Node + Bun | server, CLI, tests, Electron main, VS Code extensions |
| `@agentsync/sdk/web` | browser bundlers (Vite, webpack, Rollup, esbuild) | frontends, Tauri webview, Obsidian renderer |
| `@agentsync/sdk/wasm` | raw `.wasm` bytes | custom loaders, Cloudflare Workers |
| `@agentsync/sdk/wasm/bundler` | bundler glue + types | when you want the wasm-bindgen surface directly |

All entry points expose the same TypeScript API. The browser bundle defaults to `globalThis.WebSocket` and OPFS; Node/Bun bundles default to `ws` + `node:fs`. Pass your own adapters explicitly to override.

## Memory management

The wasm-bindgen wrappers hold pointers into linear memory. Either let `vault.close()` free everything (Vault internally tracks ownership of its `Doc` and `Identity`), or call `.free()` explicitly when working with raw primitives:

```ts
{
  using id = Identity.generate();
  // ...
}            // automatically freed if your runtime supports `using`

const doc = new Doc('v');
try {
  // ...
} finally {
  doc.free();
}
```

If you pass an externally-owned `identity` to `Vault.create` / `Vault.open`, the Vault will *not* free it — that's your responsibility.

## Develop

```bash
bun install
bun run build       # wasm-pack (bundler + nodejs targets) + tsc
bun test            # 41 unit tests (Bun)
bun run lint        # biome
bun run typecheck
AGENTSYNC_BIN=path/to/agentsync bun run test:e2e   # 5 e2e tests against a real hub
```

The e2e suite runs under Node (not Bun) because Bun's WebSocket client doesn't currently accept the hub's ed25519 self-signed cert. The five tests cover:

1. Handshake decode (sanity check on the frame codec).
2. Full TS Vault ↔ Rust hub handshake and `connected` event.
3. TS write → file appears on the hub's disk.
4. Hub write → TS Vault reads it.
5. Reconnect after the hub is killed and restarted on the same port.

## Use cases

The SDK is designed to be the same primitives across:
- **Node servers / CLI tools** — Use `nodeFsStorage` + `nodeWsTransport`.
- **Browser apps** — Use `opfsStorage` (run wasm in a Worker for sync OPFS handles); browser default transport uses `globalThis.WebSocket`. Hub must serve a real CA cert (browsers can't pin self-signed certs through `WebSocket`).
- **Electron / Tauri / Obsidian plugins** — Hybrid Node + browser environment. Pick `node` or `web` entry point depending on which process you're in; the renderer can use either since `require('@agentsync/sdk')` works in Electron's renderer.
- **VS Code / Cursor / Zed extensions** — VS Code/Cursor extensions run in Node (use the `node` entry); Zed extensions run in a WIT-sandboxed wasm host that this SDK doesn't currently support out of the box.

## Supply chain

`bunfig.toml` sets `minimumReleaseAge = 604800` so `bun install` refuses
any npm package whose latest version is less than 7 days old. This
blocks the typical short-lived poisoning window from a stolen
maintainer token before it reaches the lockfile. To bypass for a
specific incident, add the package name to `minimumReleaseAgeExcludes`
in `bunfig.toml` — don't disable globally.

## License

MIT or Apache-2.0, at your option.
