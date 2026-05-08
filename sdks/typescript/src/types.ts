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
  deleted_at: number | null;
  binary_hash: string | null;
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
