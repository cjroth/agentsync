# WIRE.md — Wire Protocol

> Normative spec. See [SPEC.md § Conformance language](./SPEC.md#conformance-language)
> for RFC 2119 keyword usage.

This document specifies the bytes on the wire between two agentsync peers.
A reimplementation that follows this document **MUST** be able to complete
a handshake with the reference implementation, exchange Automerge sync
messages, request and serve blobs, and shut down cleanly.

---

## 1. Transport

### 1.1 URL scheme

A peer connects to a hub at a URL of the form `wss://host[:port]` or
`ws://host[:port]`.

- `wss://` is TLS-terminated WebSocket. **MUST** be the default.
- `ws://` is plaintext WebSocket. Implementations **MUST** support it for
  reverse-proxy deployments where TLS termination happens upstream
  (Fly.io, Railway, Caddy). They **MUST** make plaintext opt-in (e.g., a
  `--no-tls` flag) and **SHOULD** print a clear warning.

If the URL has no port, the scheme default applies: `443` for `wss://`,
`80` for `ws://`.

If the URL has no scheme, an implementation **SHOULD** infer one (`wss://`
when TLS is on, `ws://` otherwise) so users can write `agentsync clone
hub.example` without a scheme prefix. The reference helper is
`agentsync_core::normalize_rendezvous_url`.

### 1.2 TLS

When TLS is in use:

- The hub **MUST** present a self-signed X.509 certificate over an
  ed25519 keypair. The reference uses `rcgen` to generate a 10-year cert
  on first run and persists it under
  `<storage_path>/../.agentsync-server/tls.crt` + `tls.key` (DER-encoded;
  the key file **MUST** be mode `0600`).
- The hub's TLS Common Name and SANs are unconstrained. Connecting
  clients **MUST NOT** validate hostname, expiry, or chain — TLS
  validation is delegated to the application-layer handshake's
  *channel binding* (see § 4).
- TLS protocol version **MUST** be 1.3.
- A reimplementation **MAY** delegate TLS to the underlying environment
  (browser `WebSocket`, Node `ws`); in that case the implementation
  cannot recover the cert DER and cannot perform channel binding (see
  § 4.5 for the degraded-mode rules).

### 1.3 WebSocket framing

After WebSocket upgrade, every message exchanged **MUST** be a binary
WebSocket frame containing exactly one MessagePack-encoded `Frame` (see
§ 2). Text frames are reserved and **MUST** be rejected.

Close frames **MUST** be sent with no code and no reason. The reference
sends `Message::Close(None)`. Application-level errors are propagated as
`Frame::Error` (§ 2.6) before the close.

---

## 2. Frame format

### 2.1 Encoding

Every frame is a single MessagePack object encoded with named field
tags. The reference uses `rmp-serde`'s `to_vec_named` /
`from_slice` codec.

The frame type is discriminated by a top-level field `t` (a UTF-8 string).
A reimplementation **MUST** reject any frame missing `t` or carrying an
unknown `t` value with a `Frame::Error` and close the connection.

### 2.2 Frame variants

The complete set of variants:

| `t` | Direction | Purpose |
|---|---|---|
| `hello_hub`  | hub → peer | first handshake message |
| `hello_peer` | peer → hub | second handshake message |
| `proof_hub`  | hub → peer | hub's signed proof |
| `proof_peer` | peer → hub | peer's signed proof |
| `sync`       | both       | opaque Automerge sync message |
| `blob_fetch` | both       | request a blob by hash |
| `blob_push`  | both       | send a blob by hash |
| `ping`       | both       | application-level ping |
| `pong`       | both       | response to a ping |
| `error`      | both       | terminal error notification |

Pre-handshake messages **MUST** be exactly the four `hello_*` / `proof_*`
frames in order. Any other frame received before handshake completion
**MUST** be treated as a protocol error.

### 2.3 `hello_hub`

```
{
  "t":                    "hello_hub",
  "vault_id":             string,         // vault UUID
  "hub_identity_pubkey":  bin (32 bytes), // ed25519 public key
  "hub_nonce":            bin (32 bytes), // freshly random
  "tls_cert_fingerprint": bin (32 bytes), // SHA-256 of TLS cert DER, or 32 zero bytes when TLS not in use
  "vault_name":           string | nil    // optional, may be omitted
}
```

`vault_name` **MAY** be omitted by older hubs; readers **MUST** treat
missing or `nil` as "no name advertised."

`hub_nonce` **MUST** be 32 bytes from a cryptographically secure RNG.

When TLS is not in use, `tls_cert_fingerprint` **MUST** be exactly 32
zero bytes. A reimplementation that delegates TLS to the runtime and
cannot recover the peer cert (browser WebSocket) **MUST** also send 32
zero bytes; this puts the connection in degraded channel-binding mode
(§ 4.5).

### 2.4 `hello_peer`

```
{
  "t":                    "hello_peer",
  "peer_identity_pubkey": bin (32 bytes), // ed25519 public key
  "peer_nonce":           bin (32 bytes), // freshly random
  "op":                   string          // "join" or "create"
}
```

`op` is one of the strings `"join"` or `"create"`. `"create"` is reserved
for future bootstrapping flows; v1 hubs **MUST** treat any value other
than `"join"` as an unknown operation and respond with `Frame::Error`.

`peer_nonce` **MUST** be 32 bytes from a cryptographically secure RNG.

### 2.5 `proof_hub` / `proof_peer`

```
{ "t": "proof_hub",  "sig": bin (64 bytes) }
{ "t": "proof_peer", "sig": bin (64 bytes) }
```

`sig` is a 64-byte ed25519 signature. The signing input is the *transcript*
defined in § 4.2.

### 2.6 `error`

```
{ "t": "error", "message": string }
```

Sent by either side immediately before closing the connection. The
`message` field is an opaque human-readable description. A reimplementation
**MUST NOT** rely on parsing `message`; it is for diagnostics only.

### 2.7 `sync`

```
{ "t": "sync", "bytes": bin }
```

`bytes` is an opaque Automerge sync-protocol message produced by
`automerge::sync::State::generate_sync_message()` and consumed by
`receive_sync_message()`. agentsync does not interpret these bytes.

### 2.8 `blob_fetch` / `blob_push`

```
{ "t": "blob_fetch", "hash": string }
{ "t": "blob_push",  "hash": string, "bytes": bin }
```

`hash` is the lowercase hexadecimal SHA-256 of the blob content (64
characters). On `blob_fetch`, the peer **MUST** respond with a
`blob_push` carrying the same hash, or a `Frame::Error` if the blob is
unknown locally. On `blob_push`, the receiver **MUST** verify that the
SHA-256 of `bytes` equals `hash` before persisting; mismatches **MUST**
be rejected and **SHOULD** be logged.

### 2.9 `ping` / `pong`

```
{ "t": "ping", "ts": int }
{ "t": "pong", "ts": int }
```

`ts` is an arbitrary integer (the reference uses milliseconds since the
Unix epoch as a convention, but this is not mandated). On receiving a
`ping`, a peer **MUST** respond with a `pong` echoing the same `ts`. A
peer **MAY** discard `pong` frames silently.

There is no automatic keepalive in v1. Either side **MAY** send `ping` at
its own cadence; if it does, it **SHOULD** treat the absence of `pong`
within an implementation-defined timeout as a connection failure.

---

## 3. Connection state machine

A connection moves through these states. The reference does not encode
these as an enum; they are described here for spec clarity.

```
       (TCP / TLS established + WS upgrade complete)
                      │
                      ▼
                  Pre-handshake
                      │  hub sends HelloHub
                      ▼
                  Hub-Hello-Sent  ─── peer sends HelloPeer ───┐
                                                              │
                                                              ▼
                                                        Peer-Hello-Sent
                                                          │  hub: authorized_keys check
                                                          │       (fail → Error → Closed)
                                                          ▼
                                                        Hub-Verifying
                                                          │  hub sends ProofHub
                                                          ▼
                                                        Proof-Hub-Sent
                      ┌──── peer verifies hub sig ─────────┘
                      │  (fail → Error → Closed)
                      ▼
                  Peer-Verifying
                      │  peer sends ProofPeer
                      ▼
                  Proof-Peer-Sent
                      │  hub verifies peer sig
                      │  (fail → Error → Closed)
                      ▼
                   Authenticated
                      │
                      ▼
                    Syncing
                  (sync, blob_*, ping, pong, error frames)
                      │
                      ▼
                    Closed
```

**Invariants:**

- A reimplementation **MUST NOT** emit any non-handshake frame
  (`sync`, `blob_*`, `ping`, `pong`) before reaching `Authenticated`.
- It **MAY** emit a `Frame::Error` and close from any state.
- After `Authenticated`, frames have no required ordering except those
  imposed by Automerge's sync protocol on its own bytes.

---

## 4. Handshake (normative)

### 4.1 Message order

```
hub  → peer:  HelloHub
peer → hub:   HelloPeer
hub  → peer:  ProofHub
peer → hub:   ProofPeer
```

The hub sends first. A peer **MUST NOT** send `HelloPeer` until it has
received `HelloHub`. The reference's `run_handshake` function in
`crates/agentsync-core/src/net/client.rs` is the canonical implementation
of the peer side; `handle_peer` in `net/server.rs` is the hub side.

### 4.2 Transcript

Both sides compute the same transcript and use it as the signing input
for `proof_hub` / `proof_peer`.

The transcript is the byte concatenation:

```
transcript =
    "agentsync-auth-v1"           // 17 bytes, ASCII, no terminator
 || hub_nonce                     // 32 bytes
 || peer_nonce                    // 32 bytes
 || tls_cert_fingerprint          // 32 bytes (or 32 zero bytes; see § 4.5)
 || hub_identity_pubkey           // 32 bytes
 || peer_identity_pubkey          // 32 bytes
```

The total length is **177 bytes**. A reimplementation **MUST** produce
exactly these 177 bytes in exactly this order; any other layout will
fail signature verification against the reference.

The leading 17-byte tag `agentsync-auth-v1` is a domain-separation tag
and **MUST** be present verbatim. It is not a negotiated version. See
[SPEC.md § Versioning](./SPEC.md#versioning-and-compatibility).

### 4.3 Signing

The signature algorithm is **ed25519** (RFC 8032). Each side signs the
transcript with its identity private key:

```
proof_hub.sig  = Ed25519.Sign(hub_identity_priv,  transcript)
proof_peer.sig = Ed25519.Sign(peer_identity_priv, transcript)
```

`sig` is exactly 64 bytes. A reimplementation **MAY** delegate signing to
an external agent (ssh-agent, hardware token); the agent **MUST** produce
a standard ed25519 signature over the same 177-byte input.

### 4.4 Verification

Each side verifies the *other's* signature against the same transcript:

```
hub  verifies: Ed25519.Verify(peer_identity_pubkey, transcript, proof_peer.sig)
peer verifies: Ed25519.Verify(hub_identity_pubkey,  transcript, proof_hub.sig)
```

In addition:

- The hub **MUST** check that `peer_identity_pubkey` is present in the
  vault's `authorized_keys` (see [AUTH.md § authorized_keys](./AUTH.md#authorized_keys)).
  On miss, the hub **MUST** send a `Frame::Error` whose `message`
  identifies the rejection reason (the reference uses
  `"peer pubkey {fingerprint} not authorized"`) and close the connection.
- The peer **MUST** check that the hub's identity matches its locally
  pinned `hub_pubkey` if one is configured (TOFU). On mismatch, the peer
  **MUST** abort. See [AUTH.md § Hub trust](./AUTH.md#hub-trust).
- The peer **MUST** check the channel binding (§ 4.5).

### 4.5 Channel binding

If the peer connected over TLS *and* it can recover the hub's certificate
DER from its TLS layer, then:

- Let `observed_fp = SHA-256(hub_cert_der)`.
- The peer **MUST** verify `hello_hub.tls_cert_fingerprint == observed_fp`.
- On mismatch, the peer **MUST** abort with `Frame::Error` and close.
  The reference message is
  `"tls cert fingerprint mismatch: advertised <hex>, observed <hex>"`.

If `hello_hub.tls_cert_fingerprint` is exactly 32 zero bytes, channel
binding is *disabled*. This MUST be allowed only when:

1. The connection is plaintext (`ws://`), **or**
2. The peer's transport cannot recover the cert (browser WebSocket).

In degraded-binding mode the peer **SHOULD** print a one-line warning. A
reimplementation **MAY** refuse to connect in degraded mode behind a
config flag.

The hub does not verify channel binding on its own outbound side — it
authoritatively knows its own cert. It only signs over the fingerprint
it advertised.

### 4.6 Failure handling

Any verification failure during the handshake **MUST**:

1. Be communicated to the other side as a `Frame::Error` with a
   human-readable `message`, *if and only if* the failing side has
   already received enough of the handshake to know where to send the
   error. (The hub can always send `Frame::Error` after receiving
   `HelloPeer`; the peer can send it after receiving `HelloHub`.)
2. Be followed immediately by a clean WebSocket close.

The receiving side **MUST** treat an unexpected close (i.e., one not
preceded by `Frame::Error`) as a generic protocol failure.

---

## 5. Authorized-keys enforcement

The hub **MUST** consult `authorized_keys` (a file synced through the
vault — see [DOCUMENT.md](./DOCUMENT.md) and [AUTH.md](./AUTH.md))
during the handshake (after receiving `HelloPeer`, before sending
`ProofHub`).

The hub **MUST** also re-check `authorized_keys` whenever the document
changes. If a currently-connected peer is no longer authorized, the hub
**MUST** disconnect that peer. The reference implementation runs this
check on every doc-change notification in
`crates/agentsync-core/src/net/server.rs`.

Removal is therefore eventually consistent: a peer with a stale copy of
`authorized_keys` MAY still accept a peer until the document converges.
This is documented in [AUTH.md § Threat model](./AUTH.md#threat-model)
and is a known limitation of v1.

---

## 6. Sync exchange

After the handshake completes, both sides drive Automerge's sync
protocol to convergence.

### 6.1 Initial sync

Each side, on entering the `Syncing` state, **SHOULD** call its
Automerge sync state's `generate_sync_message()` once and send the
result (if `Some`) as a `Frame::Sync`.

### 6.2 Sync loop

On receiving `Frame::Sync(bytes)`:

1. Pass `bytes` to Automerge's `receive_sync_message`.
2. If state changed, generate a reply via `generate_sync_message`.
3. If the reply is `Some`, wrap as `Frame::Sync` and send.

Either side **MAY** also generate sync messages spontaneously when its
local document changes (e.g., from a filesystem event). The reference
implementations on both sides poll their sync state with a 100 ms
timeout when waiting for changes.

A reimplementation **MUST** treat the `bytes` field as opaque — it must
not parse, modify, or filter Automerge sync messages.

### 6.3 Hub fanout

When the hub merges sync changes from peer A, it **MUST** propagate them
to all other connected peers. The reference does this by calling
`generate_sync_message` for each peer's per-peer Automerge sync state.

### 6.4 Persistence interaction

A peer **MAY** persist its Automerge document (see
[STORAGE.md § doc.bin](./STORAGE.md#docbin)) before, during, or after
sending sync messages. There is no required ordering between persistence
and the wire.

A reimplementation **SHOULD** persist its sync state per peer if it
wants offline-resumability with the same hub; the reference TypeScript
SDK does this via the `StorageAdapter`'s `loadSyncState` /
`saveSyncState` methods.

---

## 7. Blob exchange

Binary attachments larger than the inline threshold are stored as
content-addressed blobs (see [STORAGE.md § Blob store](./STORAGE.md#blob-store))
and exchanged out-of-band of the Automerge document.

When peer A's document references a blob hash that A does not have
locally, A **MAY** send `Frame::BlobFetch{hash}` to any connected peer.
The receiver **MUST**:

- If it has the blob, respond with `Frame::BlobPush{hash, bytes}`.
- If it does not, respond with `Frame::Error{message}` carrying a
  human-readable explanation.

On receiving `Frame::BlobPush{hash, bytes}`, the receiver **MUST**:

1. Verify `SHA-256(bytes) == hash` (lowercase hex).
2. If the verification passes, persist via the blob storage interface.
3. If it fails, discard and **SHOULD** log a warning.

There is no flow control on blob frames in v1. A reimplementation
**SHOULD NOT** request blobs in arbitrarily-large parallel batches; the
reference exchanges them sequentially per peer.

---

## 8. Ping / pong

Either side **MAY** send `Frame::Ping{ts}` at any time after the
handshake. The receiver **MUST** respond with `Frame::Pong{ts}` echoing
the same `ts`. The sender **MAY** use the round trip for latency
measurement or liveness detection.

Neither side is required to send pings. The reference does not generate
pings automatically; it only handles them.

---

## 9. Errors

### 9.1 Wire-level error frames

Any side **MAY** at any time after the WebSocket upgrade send
`Frame::Error{message}` followed by a clean WebSocket close. On receipt,
the other side **SHOULD** log the message and **MUST** treat the
connection as terminated.

Common error conditions and their reference messages:

| Condition | Reference message |
|---|---|
| Peer not in `authorized_keys` | `peer pubkey <fingerprint> not authorized` |
| TLS fingerprint mismatch | `tls cert fingerprint mismatch: advertised <hex>, observed <hex>` |
| Hub signature invalid | `hub signature failed verification` |
| Peer signature invalid | `peer signature failed verification` |
| Wrong frame type at handshake step | `expected HelloHub` / `expected HelloPeer` / `expected ProofHub` / `expected ProofPeer` |
| Unknown blob | `blob not found: <hash>` |
| Blob-hash mismatch | `blob hash mismatch: expected <hex>, computed <hex>` |
| Connection closed mid-handshake | `connection closed mid-handshake` |

A reimplementation's exact strings **MAY** differ; the strings are
diagnostics only and are not normatively constrained.

### 9.2 WebSocket close codes

Implementations **MUST** use a close frame with no code and no reason
(`Message::Close(None)` in the reference). Application errors are
communicated only via `Frame::Error`. A reimplementation that observes a
close frame *with* a code **SHOULD** still treat the connection as
terminated and log the code at debug level.

---

## 10. Constants

The following values are normative wire constants. A reimplementation
**MUST** match them.

| Constant | Value | Notes |
|---|---|---|
| `DEFAULT_PORT` (TLS)        | `443` | scheme default for `wss://` |
| `DEFAULT_LISTEN_ADDR`       | `0.0.0.0:443` | default with TLS |
| `DEFAULT_LISTEN_ADDR_NO_TLS`| `0.0.0.0:80`  | default without TLS |
| `HANDSHAKE_DOMAIN`          | `agentsync-auth-v1` (17 ASCII bytes) | transcript prefix |
| `NONCE_LEN`                 | `32` bytes | for both `hub_nonce` and `peer_nonce` |
| `PUBKEY_LEN`                | `32` bytes | ed25519 public key |
| `SIGNATURE_LEN`             | `64` bytes | ed25519 signature |
| `CERT_FINGERPRINT_LEN`      | `32` bytes | SHA-256 of cert DER |
| `TRANSCRIPT_LEN`            | `177` bytes | total transcript size |

The following are non-normative implementation defaults:

| Default | Reference value | Where used |
|---|---|---|
| TLS cert lifetime | 10 years | self-signed cert generation |
| Sync poll timeout | 100 ms | reference sync loop |
| WS close grace period (writer) | 2 s | reference client `close()` |
| WS close grace period (reader) | 500 ms | reference server `close()` |

A reimplementation **MAY** choose different defaults but **SHOULD**
document them.

---

## 11. Conformance vectors

The following vectors live under `specs/vectors/wire/`:

- **`vectors/wire/handshake.bin`** — captured 4-message handshake bytes
  with a fixed hub seed, peer seed, and nonces. A reimplementation can
  replay these and verify it produces matching transcripts and
  signatures.
- **`vectors/wire/transcript.bin`** — the canonical 177-byte transcript
  for a known test fixture.
- **`vectors/wire/frame-encodings.json`** — for each `Frame` variant,
  a sample value and its MessagePack hex encoding.

These are scaffolded in [vectors/README.md](./vectors/README.md).
Generation lives in `tools/gen-vectors/` (planned).

---

## 12. Cross-references

- [AUTH.md](./AUTH.md) — identity model, `authorized_keys`, hub TOFU,
  threat model.
- [STORAGE.md](./STORAGE.md) — TLS material on disk, identity files.
- [DOCUMENT.md](./DOCUMENT.md) — `authorized_keys` as a file in the
  Automerge document, blob hash references.
- [HOST.md](./HOST.md) — `Transport` / `Conn` trait used by the
  reference for portability.
