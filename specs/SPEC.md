# agentsync — Specification (v1)

> **Status:** Normative root. This document and the per-concern specs it
> indexes are authoritative for any reimplementation of agentsync. Any
> divergence between code and spec is a bug; please file it.

agentsync is a real-time, peer-to-peer sync engine for a directory of files.
A vault is one Automerge document. Peers connect over WSS to a designated
*hub* peer (any peer running with `--listen`) and converge via Automerge's
sync protocol. Authentication is per-device ed25519, gated by an SSH-style
`authorized_keys` file synced through the vault itself. Point-in-time
recovery and named labels are first-class operations on the document's
change history.

This document is the index. Every claim about *behavior* (what the bytes on
the wire are, what the on-disk file contains, what the API guarantees) lives
in one of the per-concern specs below.

## Conformance language

The keywords **MUST**, **MUST NOT**, **SHOULD**, **SHOULD NOT**, and **MAY**
in this and the linked specs are to be interpreted as in [RFC 2119][rfc2119]
and [RFC 8174][rfc8174] when, and only when, they appear in all capitals.

A statement without these keywords is descriptive — it explains rationale
or implementation behavior but is not normative. A reimplementation that
violates a "MUST" is non-conformant; a reimplementation that violates a
"SHOULD" is conformant but discouraged.

[rfc2119]: https://www.rfc-editor.org/rfc/rfc2119
[rfc8174]: https://www.rfc-editor.org/rfc/rfc8174

## Document index

| Document | Scope |
|---|---|
| [WIRE.md](./WIRE.md) | Wire protocol: framing, handshake bytes, sync exchange, error frames, connection state machine. |
| [AUTH.md](./AUTH.md) | Authentication and authorization model: identity keys, `authorized_keys`, hub TOFU, threat model. |
| [STORAGE.md](./STORAGE.md) | On-disk formats: `.agentsync/` layout, `doc.bin`, `snapshots/index.json`, `blobs/`, `config.toml`, identity files, TLS material. |
| [DOCUMENT.md](./DOCUMENT.md) | Automerge document schema: top-level keys, file tree representation, body encoding per file kind, label entries, invariants. |
| [HOST.md](./HOST.md) | Platform-abstraction contract: the `Host` trait surface a port must implement (runtime, transport, storage, filesystem, crypto). |
| [API-RUST.md](./API-RUST.md) | Public Rust API of `agentsync-core`: `Vault`, `Doc`, `Identity`, error types, stability policy. |
| [API-TS.md](./API-TS.md) | Public TypeScript API of `@agentsync/sdk`: `Vault` class, adapter interfaces, wasm boundary. |
| [vectors/](./vectors/README.md) | Golden test vectors a reimplementation can run as a conformance harness. |

## What v1 is

A vault is a directory of files synced in real time across peers, with the
following first-class properties:

1. **Per-vault Automerge document.** All file metadata, directory metadata,
   file contents, and labels live inside one Automerge document per vault.
2. **Peer-to-peer over WSS.** Every peer is the same binary. One peer runs
   with `--listen` and is reachable on a public address (the *hub*). Other
   peers connect to the hub and exchange Automerge sync messages over a
   binary WebSocket framed in MessagePack.
3. **Per-device identity.** Each device has an ed25519 keypair. The hub
   gates inbound connections by checking the connecting peer's pubkey
   against an SSH-style `authorized_keys` file stored *inside the vault*.
4. **Channel-bound authentication.** TLS uses a self-signed cert generated
   by the hub on first run; the application-layer handshake signature
   covers the SHA-256 of the cert DER, defeating active relayed MITM.
5. **History as a first-class data structure.** The Automerge document is
   the log. Labels are named pointers into that history. Restoring to a
   past moment is additive — it produces forward-going changes that bring
   the document state to match the past state.

## Goals (v1)

A v1 implementation **MUST**:

1. Sync a local directory between peers running the same binary, over WSS,
   in real time (sub-second propagation under nominal network conditions).
2. Converge concurrent edits across peers via Automerge CRDTs.
3. Authenticate every connection with ed25519 per-device identities and
   reject any peer not in the vault's `authorized_keys`.
4. Encrypt all peer-to-peer traffic with TLS 1.3.
5. Persist the full Automerge document — including history — to
   `.agentsync/doc.bin`.
6. Expose point-in-time recovery via [Automerge's][automerge] history
   primitives.
7. Expose named recovery points (labels) that themselves sync between
   peers via the document.

A v1 implementation **SHOULD**:

8. Distribute as a single static binary under 15 MB on Linux x86_64.
9. Cold-start a 10,000-file vault in under 5 seconds.
10. Run with zero external infrastructure — the "server" is the same
    binary with `--listen`.

[automerge]: https://automerge.org/

## Non-goals (v1)

These are **explicitly out of scope** for v1. A reimplementation MAY
implement them as extensions, but they are not required for conformance.

- End-to-end encryption above TLS. The hub holds vault contents in
  plaintext on its filesystem; the threat model treats the hub as a
  trusted machine the operator controls. See [AUTH.md § Threat
  model](./AUTH.md#threat-model).
- Multi-tenant SaaS hosting. A vault is a self-contained primitive with
  no concept of users, accounts, or roles.
- Per-file or per-directory access control. `authorized_keys` grants full
  read/write to the entire vault.
- Hard key revocation. Removal from `authorized_keys` is eventually
  consistent — a peer with a stale copy may still accept a removed peer
  until convergence.
- Off-peer durability for snapshots and history. Backups are the
  operator's responsibility (`restic`, `borg`, `rclone` against
  `.agentsync/`).
- Conflict-resolution UI. Automerge merges deterministically; surfacing
  the merge to a human is left to higher-level tooling.
- Per-file CRDT partitioning. v1 uses one Automerge document per vault.
- Binary file delta sync. Files large enough to not fit in the document
  are content-addressed and stored whole; see [STORAGE.md § Blob
  store](./STORAGE.md#blob-store).
- Permission systems, RBAC, mobile clients, web UI.

## Architecture

```
┌────────────────────────────────────────────────────────────┐
│                       Local Peer                            │
│                                                              │
│   Markdown files on disk                                     │
│        ↕ (notify crate / fsevents / inotify)                 │
│   ┌──────────────────────────────────────────────────────┐  │
│   │                agentsync-core (Rust)                  │  │
│   │                                                       │  │
│   │      FilesystemAdapter ↔ Vault ↔ Net                 │  │
│   │                          │                            │  │
│   │                  Automerge Document                   │  │
│   │                  (in memory, includes full history)   │  │
│   │                          ↕                            │  │
│   │                  .agentsync/  (on-disk state)         │  │
│   │                  - doc.bin (saved Automerge doc)      │  │
│   │                  - snapshots/index.json               │  │
│   │                  - blobs/<sha256> (attachments)       │  │
│   │                  - config.toml                        │  │
│   └──────────────────────────────────────────────────────┘  │
│                          ↕ wss:// (TLS 1.3)                  │
└──────────────────────────│──────────────────────────────────┘
                           │  4-message handshake, then
                           │  Automerge sync frames
                           ↓
┌──────────────────────────────────────────────────────────────┐
│        Hub Peer (agentsync --listen)                          │
│                                                                │
│   Identical binary. Bound to a public address.                 │
│   Holds the same .agentsync/ state on its own filesystem.      │
│   Generates a self-signed TLS cert on first run.               │
│   Fans out updates from one connected peer to all others.      │
└──────────────────────────────────────────────────────────────┘
                           ↑
                           │ other peers connect here
              ┌────────────┴────────────┐
              │                          │
        Other peer                 Other peer
```

Key properties:

- **Symmetric peer code.** The hub is just another peer running the same
  binary. No separate "server" codebase, no Postgres, no S3, no Docker
  Compose stack.
- **One Automerge document per vault.** Tree structure, file metadata, and
  text-file contents live inside this single document; binary attachments
  are referenced by content-addressed blob hash. Operations across files
  are atomic Automerge transactions.
- **The document is the log.** A reimplementation MUST NOT maintain a
  separate append-only log. Automerge's change DAG is the history.
- **All peer traffic is TLS 1.3.** TLS termination MAY be delegated to a
  reverse proxy (Fly.io, Railway, Caddy) by running the hub with
  `--no-tls` over plain `ws://` and binding to localhost; this is for
  deployment convenience, not security.
- **Channel-bound auth.** The handshake signature transcript covers the
  SHA-256 of the TLS cert DER. See [WIRE.md § Handshake](./WIRE.md#handshake).

## Crates and packages (current state)

agentsync ships as multiple components. A reimplementation MAY collapse
these into fewer artifacts.

| Component | Role |
|---|---|
| `agentsync-core` | Rust crate — the engine. Native targets get full functionality including networking, filesystem watching, and TLS. |
| `agentsync-wasm` | Rust crate — exposes a subset of `agentsync-core` to JavaScript via `wasm-bindgen`. |
| `agentsync-cli` | The `agentsync` binary. Thin wrapper over `agentsync-core`. |
| `@agentsync/sdk` | TypeScript package at `sdks/typescript/`. Wraps `agentsync-wasm` plus injected JS-side adapters into a high-level `Vault` class. |

See [API-RUST.md](./API-RUST.md) and [API-TS.md](./API-TS.md) for the
public surfaces.

## Versioning and compatibility

agentsync uses three independent version surfaces. A reimplementation
**MUST** track all three.

### 1. Handshake domain-separation tag

The first 17 bytes of every signed handshake transcript are the ASCII
bytes `agentsync-auth-v1`. This tag **MUST** appear verbatim. It is *not*
a negotiated version field — there is no negotiation. Any future
incompatible change to the handshake bumps this tag (e.g.,
`agentsync-auth-v2`) and is a coordinated break. See [WIRE.md §
Transcript](./WIRE.md#transcript).

### 2. Document `schema_version`

The Automerge document carries a `schema_version` integer at its root
(currently `1`). A reimplementation reading a document **MUST** check this
field; a value it does not understand is a fatal error. See
[DOCUMENT.md § Top-level keys](./DOCUMENT.md#top-level-keys).

### 3. `snapshots/index.json` `schema_version`

The on-disk snapshot index has its own `schema_version` (currently `1`).
This is local-cache state; on mismatch a reimplementation **MAY** discard
the file and rebuild from the document.

### Backward-compatibility shims

Two specific shims exist in the current code and **MUST** be preserved by
any reimplementation that wants to interoperate with existing vaults:

- **`Frame::HelloHub.vault_name`** is `Option` and serde-default; older
  hubs may omit it.
- **Legacy label encoding.** Labels stored as `ScalarValue::Bytes`
  directly (not wrapped in an object) **MUST** be readable; new labels
  **MUST** be written in the object form. See [DOCUMENT.md §
  Labels](./DOCUMENT.md#labels).

## Conformance criteria

A reimplementation is *conformant* if all of the following hold:

1. It implements every behavior marked **MUST** in this document and the
   linked specs.
2. It can complete the handshake with a reference Rust hub and exchange
   sync messages such that document state converges. The conformance test
   for this is [vectors/handshake](./vectors/README.md).
3. It can load a `doc.bin` produced by the reference Rust implementation,
   and a `doc.bin` it produces can be loaded by the reference. The
   conformance test for this is [vectors/doc-roundtrip](./vectors/README.md).
4. It rejects (with the appropriate `Frame::Error`) any peer not in
   `authorized_keys`.
5. It writes `snapshots/index.json` and `config.toml` in the schema
   defined by [STORAGE.md](./STORAGE.md), such that the reference
   implementation can read them.

A reimplementation that targets only a *subset* of the API (e.g., a
read-only browser client without filesystem watching) is conformant for
that subset if it satisfies the relevant items above. The test-vector
manifest declares which vectors apply to which subset.

## Implementation references

The reference implementation lives in this repository. When the spec is
ambiguous, the reference implementation is authoritative — and the
ambiguity is a spec bug.

- Rust core: `crates/agentsync-core/`
- Wasm bridge: `crates/agentsync-wasm/`
- CLI: `crates/agentsync-cli/`
- TypeScript SDK: `sdks/typescript/`
- E2E tests: `tests/`
- Test vectors: `specs/vectors/`

## Spec maintenance

Any change to wire format, on-disk format, or document schema **MUST**
include a paired update to the relevant per-concern spec in the same
commit or pull request. Reviewers should reject changes that leave the
spec stale.

Test vectors **MUST** be regenerated whenever the wire or storage format
changes intentionally; tools for this live in
`specs/vectors/`. See [vectors/README.md](./vectors/README.md).

## Open issues / non-normative roadmap

The following items are described in the original product spec and are
*planned* but not yet specified normatively. They are listed here so the
roadmap is visible to reimplementers without polluting the normative
specs.

- **Compaction.** `agentsync compact` to drop history older than a
  retention window. Currently a milestone item; no spec exists.
- **Blob garbage collection.** Blobs in `.agentsync/blobs/` are never
  reclaimed in v1. A future GC pass keyed on live `binary_hash` references
  is anticipated.
- **`agentsync diff <heads-or-time> <heads-or-time>`** to inspect changes
  between two history points.
- **Performance budget enforcement in CI.** A regression > 20% on any
  named benchmark fails CI. Benchmarks themselves are not specified
  normatively.

These are not part of the conformance bar for v1.
