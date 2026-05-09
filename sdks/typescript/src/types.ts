// JS-side mirrors of the agentsync-core data shapes that come back as
// plain objects from the wasm boundary (via serde-wasm-bindgen).

export interface AuthorizedPeer {
  /** SSH-style public key, e.g. `ssh-ed25519 AAAA...` */
  pubkey: string;
  /** Optional human-readable label (`alice`, `homelab-nas`, etc). */
  label: string;
}

export interface FileMeta {
  id: string;
  path: string;
  kind: 'Text' | 'Attachment';
  size: number;
  created_at: number;
  updated_at: number;
  /** Soft-delete timestamp (Unix ms). Missing/undefined when the file is alive. */
  deleted_at?: number | null;
  /** Hex SHA-256 of attachment bytes; missing for text files. */
  binary_hash?: string | null;
}

/** Tag of every wire frame; matches `Frame::t` in the Rust enum. */
export type FrameTag =
  | 'hello_hub'
  | 'hello_peer'
  | 'proof_hub'
  | 'proof_peer'
  | 'sync'
  | 'blob_fetch'
  | 'blob_push'
  | 'ping'
  | 'pong'
  | 'error';

export type HelloOp = 'join' | 'create';

/**
 * Discriminated union mirroring the Rust `Frame` enum. Decoded frames come
 * back from `decodeFrame` with the `t` tag set, and `encodeFrame` accepts
 * the same shape.
 */
export type Frame =
  | {
      t: 'hello_hub';
      vault_id: string;
      hub_identity_pubkey: Uint8Array;
      hub_nonce: Uint8Array;
      tls_cert_fingerprint: Uint8Array;
      vault_name?: string | null;
    }
  | {
      t: 'hello_peer';
      peer_identity_pubkey: Uint8Array;
      peer_nonce: Uint8Array;
      op: HelloOp;
    }
  | { t: 'proof_hub'; sig: Uint8Array }
  | { t: 'proof_peer'; sig: Uint8Array }
  | { t: 'sync'; bytes: Uint8Array }
  | { t: 'blob_fetch'; hash: string }
  | { t: 'blob_push'; hash: string; bytes: Uint8Array }
  | { t: 'ping'; ts: number }
  | { t: 'pong'; ts: number }
  | { t: 'error'; message: string };

/** Snapshot label as returned by `Doc.listLabels()`. */
export interface Label {
  name: string;
  /** Heads encoded as base64 (no padding) — pair-wise concatenation of 32-byte hashes. */
  heads_b64: string;
  created_at_ms: number;
}

/** Directory metadata. */
export interface DirectoryMeta {
  id: string;
  path: string;
  created_at: number;
  deleted_at?: number | null;
}

/** High-level event emitted by a `Vault`. */
export type VaultEvent =
  | { kind: 'connecting'; url: string }
  | { kind: 'connected'; hub_pubkey: Uint8Array; vault_id: string }
  | { kind: 'disconnected'; reason: string }
  | { kind: 'sync-progress'; outbound: boolean }
  | { kind: 'doc-changed'; heads: Uint8Array[] }
  | { kind: 'error'; message: string };

/** Storage adapter contract. The wasm crate doesn't implement storage —
 * the TS SDK ships OPFS / Node FS / in-memory adapters and consumers can
 * supply their own. */
export interface StorageAdapter {
  /** Load `doc.bin` bytes; resolve to null when no doc has been saved. */
  loadDoc(): Promise<Uint8Array | null>;
  /** Atomically replace `doc.bin`. */
  saveDoc(bytes: Uint8Array): Promise<void>;
  /** Sync state per peer; key is hex pubkey. */
  loadSyncState(peerKey: string): Promise<Uint8Array | null>;
  saveSyncState(peerKey: string, bytes: Uint8Array): Promise<void>;
  /** Optional persistent identity seed. */
  loadIdentitySeed(): Promise<Uint8Array | null>;
  saveIdentitySeed(seed: Uint8Array): Promise<void>;
  /** Snapshot index (label list) — JSON or any opaque bytes. */
  loadSnapshots(): Promise<Uint8Array | null>;
  saveSnapshots(bytes: Uint8Array): Promise<void>;
  /** Best-effort dispose — release file handles, close DB connections, etc. */
  close(): Promise<void>;
}

/** Transport contract — JS side picks `ws` (Node) or native `WebSocket`
 * (browser) or anything else that speaks binary WebSocket frames. */
export interface TransportAdapter {
  connect(url: string, opts?: TransportConnectOpts): Promise<TransportConn>;
}

export interface TransportConnectOpts {
  /** Rejected if the WebSocket reports a peer cert that doesn't match. */
  pinnedCertFingerprint?: Uint8Array;
}

export interface TransportConn {
  /** Send one binary frame. */
  send(bytes: Uint8Array): Promise<void>;
  /** Async iterable of inbound frames; ends when the peer closes. */
  recv(): AsyncIterable<Uint8Array>;
  /** TLS peer cert SHA-256 if the runtime exposes it. Browsers can't. */
  channelBinding(): Uint8Array | null;
  close(): Promise<void>;
}

/** Options accepted by `Vault.create` / `Vault.open`. */
export interface VaultOptions {
  /** Persistent state root. */
  storage: StorageAdapter;
  /** Optional identity (generated if absent). */
  identity?: import('./wrapper.js').Identity;
  /** Hub URL, e.g. `wss://hub.example.com`. Required for sync; optional for offline use. */
  rendezvousUrl?: string;
  /** Pin the hub's identity pubkey (TOFU). */
  hubPubkey?: Uint8Array;
  /** Display name carried in the handshake. */
  name?: string;
  /** WebSocket transport. Defaults to the runtime-appropriate adapter. */
  transport?: TransportAdapter;
}

export interface ReconnectOptions {
  /** Total attempts before giving up (default: Infinity). */
  maxAttempts?: number;
  /** Initial backoff in ms (default: 500). */
  initialBackoffMs?: number;
  /** Max backoff in ms (default: 30000). */
  maxBackoffMs?: number;
}
