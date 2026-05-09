// High-level Vault — the user-facing TS API. Mirrors `core::Vault` from the
// Rust SDK as closely as the runtime allows: connect / connectWithReconnect
// / disconnect, write/read/delete/list files + directories, label
// snapshots, restore-to-label / restore-to-time, doc-changed events.
//
// Networking and persistence live in injectable adapters (`StorageAdapter`,
// `TransportAdapter`) so the same Vault works in browsers (OPFS + native
// WebSocket), Node/Bun (node:fs + ws), Electron/Tauri/Obsidian (whichever
// adapters the host plugs in).
//
// This module owns the protocol state machine: WebSocket framing, the
// 4-message handshake, the Automerge incremental sync loop, and the
// reconnect/backoff supervisor. The wasm crate provides the cryptographic
// + CRDT primitives; everything else is plain TS that runs anywhere.

import type {
  Frame,
  Label,
  ReconnectOptions,
  StorageAdapter,
  TransportAdapter,
  TransportConn,
  VaultEvent,
  VaultOptions,
} from './types.js';
import type { Doc, Identity, Pubkey, SyncState } from './wrapper.js';

interface WasmBindings {
  Doc: { new (vaultId: string): Doc; load(bytes: Uint8Array): Doc };
  Identity: { generate(): Identity; fromSeed(seed: Uint8Array): Identity };
  Pubkey: { fromBytes(bytes: Uint8Array): Pubkey };
  SyncState: { new (): SyncState; decode(bytes: Uint8Array): SyncState };
  buildTranscript: (
    hubNonce: Uint8Array,
    peerNonce: Uint8Array,
    tlsCertFingerprint: Uint8Array,
    hubPubkey: Uint8Array,
    peerPubkey: Uint8Array,
  ) => Uint8Array;
  randomNonce: () => Uint8Array;
  encodeFrame: (frame: Frame) => Uint8Array;
  decodeFrame: (bytes: Uint8Array) => Frame;
  contentHash: (bytes: Uint8Array) => string;
}

interface ConnectionState {
  conn: TransportConn;
  hubPubkey: Uint8Array;
  syncState: SyncState;
  /** Set true once the handshake completes; subsequent frames are sync. */
  ready: boolean;
}

const HANDSHAKE_TIMEOUT_MS = 15_000;

/** Internal: one-byte equality check for Uint8Arrays. */
function byteEq(a: Uint8Array, b: Uint8Array): boolean {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
  return true;
}

function hexOf(bytes: Uint8Array): string {
  return Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');
}

export interface CreateOptions extends VaultOptions {}
export interface OpenOptions extends VaultOptions {}

/**
 * The high-level entry point. `Vault.create` initializes a fresh vault
 * (generates a vault_id, seeds `authorized_keys` with the creator's
 * pubkey). `Vault.open` reuses an existing one.
 *
 * Exactly one Vault instance per (storage, identity) pair — running two
 * concurrent Vaults against the same storage will produce conflicting
 * doc.bin writes.
 */
export class Vault {
  private wasm: WasmBindings;
  private opts: VaultOptions;
  private doc: Doc;
  private identity: Identity;
  private vaultId: string;
  /** True when we generated/loaded the identity ourselves and should free
   * it on close. False when the caller passed one in — they own it. */
  private ownsIdentity: boolean;
  private connection: ConnectionState | null = null;
  private reconnectAbort: AbortController | null = null;
  private listeners = new Set<(e: VaultEvent) => void>();
  private closed = false;

  /** Create a brand-new vault on the supplied storage. Pass `vaultId` to
   * adopt an existing remote vault (the joining-an-existing-hub case);
   * in that mode no authorized_keys is seeded — the joining peer learns
   * everything from sync. */
  static async create(wasm: WasmBindings, options: CreateOptions): Promise<Vault> {
    const ownsIdentity = !options.identity;
    const identity = options.identity ?? (await loadOrCreateIdentity(wasm, options.storage));
    const joiningExisting = !!options.vaultId;
    const vaultId = options.vaultId ?? generateVaultId();
    const doc = new wasm.Doc(vaultId);
    if (!joiningExisting) {
      // Fresh vault: seed authorized_keys with the creator's pubkey so
      // the creator can connect to their own listener immediately.
      // Mirrors `Vault::create` in the Rust core.
      const pk = identity.pubkey();
      const sshLine = `${pk.toSshString()} creator\n`;
      doc.writeTextFile('authorized_keys', sshLine);
      pk.free();
    }
    const bytes = doc.save();
    await options.storage.saveDoc(bytes);
    return new Vault(wasm, options, doc, identity, vaultId, ownsIdentity);
  }

  /** Open an existing vault from storage; errors if no doc exists. */
  static async open(wasm: WasmBindings, options: OpenOptions): Promise<Vault> {
    const ownsIdentity = !options.identity;
    const identity = options.identity ?? (await loadOrCreateIdentity(wasm, options.storage));
    const bytes = await options.storage.loadDoc();
    if (!bytes) {
      throw new Error('no vault on disk; call Vault.create() first');
    }
    const doc = wasm.Doc.load(bytes);
    const vaultId = doc.vaultId();
    return new Vault(wasm, options, doc, identity, vaultId, ownsIdentity);
  }

  private constructor(
    wasm: WasmBindings,
    opts: VaultOptions,
    doc: Doc,
    identity: Identity,
    vaultId: string,
    ownsIdentity: boolean,
  ) {
    this.wasm = wasm;
    this.opts = opts;
    this.doc = doc;
    this.identity = identity;
    this.vaultId = vaultId;
    this.ownsIdentity = ownsIdentity;
  }

  // ---- Read-only accessors ----

  vaultIdValue(): string {
    return this.vaultId;
  }
  identityRef(): Identity {
    return this.identity;
  }
  isConnected(): boolean {
    return this.connection?.ready ?? false;
  }

  // ---- File operations (delegate to Doc, persist + push sync after) ----

  async writeTextFile(path: string, content: string): Promise<string> {
    const id = this.doc.writeTextFile(path, content);
    await this.flush();
    await this.kickSyncLoop();
    return id;
  }

  async readTextFile(path: string): Promise<string> {
    return this.doc.readFile(path);
  }

  fileExists(path: string): boolean {
    return this.doc.fileExists(path);
  }

  async deleteFile(path: string): Promise<void> {
    this.doc.deleteFile(path);
    await this.flush();
    await this.kickSyncLoop();
  }

  async renameFile(from: string, to: string): Promise<void> {
    this.doc.renameFile(from, to);
    await this.flush();
    await this.kickSyncLoop();
  }

  listFiles() {
    return this.doc.listFiles();
  }

  async createDirectory(path: string): Promise<string> {
    const id = this.doc.createDirectory(path);
    await this.flush();
    await this.kickSyncLoop();
    return id;
  }

  async deleteDirectory(path: string, recursive = false): Promise<void> {
    this.doc.deleteDirectory(path, recursive);
    await this.flush();
    await this.kickSyncLoop();
  }

  listDirectories() {
    return this.doc.listDirectories();
  }

  // ---- Labels / restore ----

  async createLabel(name: string): Promise<void> {
    this.doc.createLabel(name);
    await this.flush();
  }
  async deleteLabel(name: string): Promise<void> {
    this.doc.deleteLabel(name);
    await this.flush();
  }
  listLabels(): Label[] {
    return this.doc.listLabels();
  }
  async restoreToLabel(name: string): Promise<void> {
    this.doc.restoreToLabel(name);
    await this.flush();
    await this.kickSyncLoop();
  }
  async restoreToTime(targetMs: number): Promise<void> {
    this.doc.restoreToTime(targetMs);
    await this.flush();
    await this.kickSyncLoop();
  }

  // ---- Events ----

  /** Subscribe to vault events. Returns an unsubscribe function. */
  subscribe(listener: (e: VaultEvent) => void): () => void {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  }

  /** Async-iterable view of events. */
  async *events(): AsyncIterableIterator<VaultEvent> {
    const queue: VaultEvent[] = [];
    let resolveNext: ((v: VaultEvent | null) => void) | null = null;
    const unsub = this.subscribe((e) => {
      if (resolveNext) {
        const r = resolveNext;
        resolveNext = null;
        r(e);
      } else {
        queue.push(e);
      }
    });
    try {
      while (!this.closed) {
        if (queue.length > 0) {
          yield queue.shift()!;
          continue;
        }
        const next = await new Promise<VaultEvent | null>((res) => {
          resolveNext = res;
        });
        if (next === null) return;
        yield next;
      }
    } finally {
      unsub();
    }
  }

  private emit(e: VaultEvent) {
    for (const l of this.listeners) {
      try {
        l(e);
      } catch {
        // listener throws don't propagate
      }
    }
  }

  // ---- Connection management ----

  /** Connect once, run the handshake + sync loop, return when the
   * connection closes (cleanly or with error). Use `connectWithReconnect`
   * for production. */
  async connect(): Promise<void> {
    const url = this.opts.rendezvousUrl;
    if (!url) throw new Error('rendezvousUrl is required to connect');
    const transport = this.opts.transport ?? defaultTransport();
    this.emit({ kind: 'connecting', url });
    const conn = await transport.connect(url);
    try {
      const result = await this.runHandshake(conn);
      this.connection = result;
      this.emit({
        kind: 'connected',
        hub_pubkey: result.hubPubkey,
        vault_id: this.vaultId,
      });
      await this.runSyncLoop(result);
    } finally {
      try {
        await conn.close();
      } catch {}
      this.connection = null;
      this.emit({ kind: 'disconnected', reason: 'connection closed' });
    }
  }

  /** Connect with exponential backoff. Resolves when the supervisor is
   * told to stop via `disconnect()`. */
  async connectWithReconnect(opts: ReconnectOptions = {}): Promise<void> {
    const max = opts.maxAttempts ?? Number.POSITIVE_INFINITY;
    const initial = opts.initialBackoffMs ?? 500;
    const cap = opts.maxBackoffMs ?? 30_000;
    this.reconnectAbort = new AbortController();
    const signal = this.reconnectAbort.signal;
    let attempt = 0;
    while (!signal.aborted && attempt < max) {
      attempt += 1;
      try {
        await this.connect();
        attempt = 0; // reset on clean exit
      } catch (e) {
        if (signal.aborted) return;
        const delay = Math.min(initial * 2 ** Math.min(attempt - 1, 20), cap);
        this.emit({
          kind: 'error',
          message: `connect failed (attempt ${attempt}): ${e}`,
        });
        await sleep(delay, signal);
      }
    }
  }

  /** Stop the reconnect supervisor and tear down the active connection. */
  async disconnect(): Promise<void> {
    this.reconnectAbort?.abort();
    this.reconnectAbort = null;
    if (this.connection) {
      try {
        await this.connection.conn.close();
      } catch {}
      this.connection = null;
    }
  }

  /** Final cleanup: persist, drop the connection, free wasm memory. */
  async close(): Promise<void> {
    if (this.closed) return;
    this.closed = true;
    await this.disconnect();
    await this.flush();
    try {
      this.doc.free();
    } catch {}
    if (this.ownsIdentity) {
      try {
        this.identity.free();
      } catch {}
    }
    await this.opts.storage.close();
  }

  // ---- Internal: protocol state machine ----

  private async runHandshake(conn: TransportConn): Promise<ConnectionState> {
    const recvIter = conn.recv()[Symbol.asyncIterator]();
    const nextFrame = async (): Promise<Frame> => {
      const r = await withTimeout(recvIter.next(), HANDSHAKE_TIMEOUT_MS);
      if (r.done) throw new Error('connection closed during handshake');
      return this.wasm.decodeFrame(r.value);
    };

    // 1. Hub → Peer: HelloHub
    const helloHub = await nextFrame();
    if (helloHub.t !== 'hello_hub') {
      throw new Error(`expected hello_hub, got ${helloHub.t}`);
    }
    if (helloHub.vault_id !== this.vaultId) {
      throw new Error(`vault_id mismatch: hub=${helloHub.vault_id} local=${this.vaultId}`);
    }
    if (this.opts.hubPubkey && !byteEq(this.opts.hubPubkey, helloHub.hub_identity_pubkey)) {
      throw new Error('hub pubkey does not match pinned value');
    }
    const channelBinding = conn.channelBinding() ?? new Uint8Array(0);
    if (
      helloHub.tls_cert_fingerprint.length > 0 &&
      channelBinding.length > 0 &&
      !byteEq(helloHub.tls_cert_fingerprint, channelBinding)
    ) {
      throw new Error('tls cert fingerprint mismatch (channel binding)');
    }

    // 2. Peer → Hub: HelloPeer
    const peerNonce = this.wasm.randomNonce();
    const peerPk = this.identity.pubkey();
    const peerPkBytes = peerPk.bytes();
    peerPk.free();
    await this.sendFrame(conn, {
      t: 'hello_peer',
      peer_identity_pubkey: peerPkBytes,
      peer_nonce: peerNonce,
      op: 'join',
    });

    // 3. Hub → Peer: ProofHub
    const proofHub = await nextFrame();
    if (proofHub.t !== 'proof_hub') {
      throw new Error(`expected proof_hub, got ${proofHub.t}`);
    }
    const transcript = this.wasm.buildTranscript(
      helloHub.hub_nonce,
      peerNonce,
      helloHub.tls_cert_fingerprint,
      helloHub.hub_identity_pubkey,
      peerPkBytes,
    );
    const hubPk = this.wasm.Pubkey.fromBytes(helloHub.hub_identity_pubkey);
    const ok = hubPk.verify(transcript, proofHub.sig);
    hubPk.free();
    if (!ok) throw new Error('hub signature verification failed');

    // 4. Peer → Hub: ProofPeer
    const peerSig = await this.identity.sign(transcript);
    await this.sendFrame(conn, { t: 'proof_peer', sig: peerSig });

    // Set up incremental sync state. Loaded from storage if we've talked to
    // this hub before; otherwise fresh.
    const stateKey = hexOf(helloHub.hub_identity_pubkey);
    const savedState = await this.opts.storage.loadSyncState(stateKey);
    const syncState = savedState
      ? this.wasm.SyncState.decode(savedState)
      : new this.wasm.SyncState();

    const result: ConnectionState = {
      conn,
      hubPubkey: helloHub.hub_identity_pubkey,
      syncState,
      ready: true,
    };
    return result;
  }

  private async runSyncLoop(state: ConnectionState): Promise<void> {
    // Drive an initial outbound message.
    await this.pumpOutbound(state);

    for await (const bytes of state.conn.recv()) {
      const frame = this.wasm.decodeFrame(bytes);
      switch (frame.t) {
        case 'sync': {
          const moved = this.doc.receiveSyncMessage(state.syncState, frame.bytes);
          if (moved) {
            await this.flush();
            this.emit({ kind: 'doc-changed', heads: this.doc.heads() });
          }
          await this.pumpOutbound(state);
          await this.persistSyncState(state);
          break;
        }
        case 'ping':
          await this.sendFrame(state.conn, { t: 'pong', ts: frame.ts });
          break;
        case 'pong':
          break;
        case 'blob_fetch':
        case 'blob_push':
          // Not supported in storage-only mode; ignore for now. Full blob
          // support requires a JS-side blob CAS — out of scope for v1.
          break;
        case 'error':
          this.emit({ kind: 'error', message: frame.message });
          return;
        default:
          // Late handshake frame after handshake completed — ignore.
          break;
      }
    }
  }

  /** Send any pending sync messages. Called on heads change + on inbound. */
  private async pumpOutbound(state: ConnectionState): Promise<void> {
    while (true) {
      const msg = this.doc.generateSyncMessage(state.syncState);
      if (!msg) return;
      await this.sendFrame(state.conn, { t: 'sync', bytes: msg });
      this.emit({ kind: 'sync-progress', outbound: true });
    }
  }

  private async sendFrame(conn: TransportConn, frame: Frame): Promise<void> {
    const bytes = this.wasm.encodeFrame(frame);
    await conn.send(bytes);
  }

  /** Wake the sync loop after a local doc mutation. No-op when offline.
   * Awaitable so callers can ensure the change is on the wire before they
   * return. (The sync loop also pumps on every inbound; this kick is what
   * delivers a local-only change while no inbound is in flight.) */
  private async kickSyncLoop(): Promise<void> {
    if (this.connection?.ready) {
      await this.pumpOutbound(this.connection);
    }
  }

  /** Persist doc + sync state. Called after every mutation + on inbound
   * sync-message-applied. Cheap because Automerge.save is incremental-ish;
   * the storage adapter writes atomically. */
  private async flush(): Promise<void> {
    if (this.closed) return;
    const bytes = this.doc.save();
    await this.opts.storage.saveDoc(bytes);
    if (this.connection?.ready) await this.persistSyncState(this.connection);
  }

  private async persistSyncState(state: ConnectionState): Promise<void> {
    const bytes = state.syncState.encode();
    await this.opts.storage.saveSyncState(hexOf(state.hubPubkey), bytes);
  }
}

// ---- Helpers ----

function generateVaultId(): string {
  // RFC 4122 v4 UUID via crypto.getRandomValues. Available in Node ≥ 19,
  // Bun, all modern browsers.
  const bytes = new Uint8Array(16);
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const c: any = globalThis.crypto;
  c.getRandomValues(bytes);
  bytes[6] = (bytes[6]! & 0x0f) | 0x40;
  bytes[8] = (bytes[8]! & 0x3f) | 0x80;
  const hex = hexOf(bytes);
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

async function loadOrCreateIdentity(
  wasm: WasmBindings,
  storage: StorageAdapter,
): Promise<Identity> {
  const seed = await storage.loadIdentitySeed();
  if (seed) return wasm.Identity.fromSeed(seed);
  const id = wasm.Identity.generate();
  await storage.saveIdentitySeed(id.seed());
  return id;
}

function defaultTransport(): TransportAdapter {
  // Resolved at call time so import-time evaluation in browsers doesn't
  // try to require('ws'). Consumers should explicitly pass `transport` if
  // they need full control; this best-effort autodetect handles the
  // simple cases.
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const w: any = (globalThis as any).WebSocket;
  if (typeof w === 'function') {
    return makeBrowserTransport(w);
  }
  throw new Error(
    'no WebSocket implementation found; pass `transport` explicitly or import from @agentsync/sdk/node',
  );
}

function makeBrowserTransport(WebSocketCtor: typeof globalThis.WebSocket): TransportAdapter {
  return {
    async connect(url: string): Promise<TransportConn> {
      const ws = new WebSocketCtor(url);
      ws.binaryType = 'arraybuffer';
      await new Promise<void>((res, rej) => {
        ws.addEventListener('open', () => res(), { once: true });
        ws.addEventListener('error', () => rej(new Error(`websocket error connecting to ${url}`)), {
          once: true,
        });
      });
      const incoming: Uint8Array[] = [];
      let waiter: ((v: Uint8Array | null) => void) | null = null;
      let closed = false;
      ws.addEventListener('message', (ev) => {
        const data = ev.data instanceof ArrayBuffer ? new Uint8Array(ev.data) : null;
        if (!data) return;
        if (waiter) {
          const w = waiter;
          waiter = null;
          w(data);
        } else {
          incoming.push(data);
        }
      });
      ws.addEventListener('close', () => {
        closed = true;
        if (waiter) {
          const w = waiter;
          waiter = null;
          w(null);
        }
      });
      return {
        async send(bytes: Uint8Array) {
          ws.send(bytes);
        },
        async *recv() {
          while (true) {
            if (incoming.length > 0) {
              yield incoming.shift()!;
              continue;
            }
            if (closed) return;
            const next = await new Promise<Uint8Array | null>((res) => {
              waiter = res;
            });
            if (next === null) return;
            yield next;
          }
        },
        channelBinding(): Uint8Array | null {
          // Browsers don't expose peer cert; channel binding falls back to
          // empty in the handshake. The hub must be on a real CA cert for
          // this mode to be safe.
          return null;
        },
        async close() {
          ws.close();
        },
      };
    },
  };
}

async function sleep(ms: number, signal?: AbortSignal): Promise<void> {
  return new Promise((res) => {
    const t = setTimeout(res, ms);
    if (signal) {
      const onAbort = () => {
        clearTimeout(t);
        res();
      };
      if (signal.aborted) onAbort();
      else signal.addEventListener('abort', onAbort, { once: true });
    }
  });
}

async function withTimeout<T>(p: Promise<T>, ms: number): Promise<T> {
  let to: ReturnType<typeof setTimeout>;
  return await Promise.race([
    p.finally(() => clearTimeout(to)),
    new Promise<T>((_, rej) => {
      to = setTimeout(() => rej(new Error(`timeout after ${ms}ms`)), ms);
    }),
  ]);
}
