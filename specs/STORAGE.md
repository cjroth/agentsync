# STORAGE.md — On-Disk Formats

> Normative spec. See [SPEC.md § Conformance language](./SPEC.md#conformance-language)
> for RFC 2119 keyword usage.

This document specifies every byte agentsync writes to disk. A
reimplementation that follows this document **MUST** be able to
read a `.agentsync/` directory produced by the reference and produce
one the reference can read.

---

## 1. Vault directory layout

A vault is a directory chosen by the user. Inside it, agentsync owns
one subdirectory: `.agentsync/`. The user's files live alongside it.

```
my-vault/
├── notes/
│   ├── research.md          ← user file (synced)
│   └── todo.md              ← user file (synced)
├── README.md                ← user file (synced)
├── authorized_keys          ← synced via the document; see § 7
└── .agentsync/              ← agentsync-managed, MUST NOT be edited by hand
    ├── config.toml          ← per-vault config (§ 6)
    ├── doc.bin              ← saved Automerge document (§ 2)
    ├── snapshots/
    │   └── index.json       ← labels cache (§ 3)
    └── blobs/
        └── <sha256-hex>     ← content-addressed binary attachments (§ 4)
```

A reimplementation **MUST** use exactly these paths (relative to the
vault root and `.agentsync/` subdirectory) so vaults are portable
between implementations.

The hub additionally owns a *sibling* directory `<vault-parent>/.agentsync-server/`
holding TLS material; see § 5.

The user's identity key is stored *outside* any vault, by default at
`~/.agentsync/id_ed25519`. See § 8.

---

## 2. `doc.bin`

### 2.1 Format

`.agentsync/doc.bin` is the saved Automerge document for the vault. The
byte format is whatever `automerge::AutoCommit::save()` produces — a
columnar binary encoding that includes the full change history.

A reimplementation **MUST** load this file via Automerge's `load`
constructor and **MUST** produce the file via `save` (or
`save_incremental` after a full save).

Logical schema of the document is specified in [DOCUMENT.md](./DOCUMENT.md).

### 2.2 Persistence cadence

The reference saves under either of two conditions, whichever fires
first:

- **Time-based:** every 1 second, if the document changed since the
  last save.
- **Change-based:** after 100 accumulated Automerge changes.

A reimplementation **MAY** use different thresholds. It **MUST** save:

- on clean shutdown,
- before disconnecting (so peers don't see acknowledged changes that are
  later lost), and
- when the caller explicitly asks (e.g., a `flush()` API).

### 2.3 Atomic write

Every write to `doc.bin` **MUST** be atomic. The reference uses:

1. Write the full saved bytes to `<storage>/doc.bin.tmp`.
2. `rename(doc.bin.tmp, doc.bin)`.

The temporary file name **MUST** be `doc.bin.tmp` (in the same directory
as `doc.bin`, so the rename is on the same filesystem). A
reimplementation **MAY** call `fsync` before the rename for added
durability; the reference does not.

If the process crashes between (1) and (2), the implementation **MUST**
recover by loading the existing `doc.bin`. Any unflushed in-memory
changes are lost; this is acceptable because they were not yet
acknowledged to peers.

### 2.4 Save format choice

Implementations **MAY** use Automerge's `save_incremental()` for
performance. The on-disk file is still produced by full `save()` — the
reference re-saves the whole document each cycle and does not maintain
an external append log.

---

## 3. `snapshots/index.json`

### 3.1 Purpose

A local cache of the vault's labels (named recovery points). The
authoritative copy lives inside the Automerge document — see
[DOCUMENT.md § Labels](./DOCUMENT.md#labels). This file is for fast
reads without having to load the document.

### 3.2 Schema

```json
{
  "schema_version": 1,
  "labels": [
    {
      "label": "<string>",
      "heads": "<base64-no-pad of concatenated 32-byte ChangeHash values>",
      "created_at": <integer milliseconds since Unix epoch>
    },
    ...
  ]
}
```

Field requirements:

- `schema_version` **MUST** equal `1` for v1. A reader encountering a
  different value **MAY** discard the file and rebuild from the
  document.
- `labels` is an array, in any order; the reference sorts by `label`
  name when writing, but readers **MUST NOT** rely on order.
- `label` is the human-readable label name (a UTF-8 string).
- `heads` is the base64 (RFC 4648 "standard alphabet, no padding")
  encoding of `N * 32` raw bytes, where `N` is the number of Automerge
  change-hash values comprising the label. Implementations **MUST**
  reject a `heads` whose decoded length is not a multiple of 32.
- `created_at` is the wall-clock millisecond timestamp at which the
  label was created.

### 3.3 Atomic write

Same protocol as `doc.bin`:

1. Write to `<storage>/snapshots/index.json.tmp`.
2. `rename` to `<storage>/snapshots/index.json`.

The directory `snapshots/` **MUST** be created if absent before writing.

### 3.4 Read semantics

If the file is absent, a reimplementation **MUST** treat it as an empty
labels list (not as an error). This keeps fresh-clone behavior simple.

### 3.5 Drift from document

The on-disk index is a *cache*. The document is the source of truth.
After loading the document, an implementation **SHOULD** rewrite the
index from the document's labels map to keep them in sync. The
reference does this on every label-affecting operation.

---

## 4. Blob store

### 4.1 Layout

```
.agentsync/blobs/
├── <hex-sha256>          ← raw bytes of the blob
├── <hex-sha256>
└── ...
```

### 4.2 Naming

Each filename **MUST** be the lowercase hexadecimal SHA-256 of the
blob's bytes — exactly 64 hex characters, no extension, no prefix
sharding, no separator.

Example: `e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855`
is the empty blob.

### 4.3 Atomic write

To write a new blob:

1. Compute `hash = lowercase_hex(SHA-256(bytes))`.
2. If `<storage>/blobs/<hash>` already exists, the write is a no-op.
3. Otherwise, write `bytes` to `<storage>/blobs/.tmp.<hash>`.
4. `rename(.tmp.<hash>, <hash>)`.

The temp filename **MUST** start with `.tmp.` so it is distinguishable
from a real blob if the rename fails partway and a stale file is left
behind (a reimplementation **MAY** sweep `.tmp.*` files at startup).

### 4.4 Verified write

When a blob arrives over the wire (via `blob_push` — see
[WIRE.md § 7](./WIRE.md#7-blob-exchange)), the implementation **MUST**
re-verify the SHA-256 before persisting. The reference's `put_with_hash`
function performs this check.

### 4.5 What goes in blobs vs the document

This decision is made at the *binding* layer (the filesystem-watch
adapter), not by the storage layer. The reference rule is:

- Files whose extension is in `[sync] extensions` are stored as
  Automerge `Text` *inside* the document. They are subject to
  `[sync] text_file_max_bytes` (default 1 MiB).
- All other allowed files are stored as blobs and referenced by hash
  in the document. They are subject to `[sync] attachment_max_bytes`
  (default 10 MiB).

Both limits are enforced *before* writing to the document or the blob
store; oversized files **MUST** be rejected with a clear error.

### 4.6 Garbage collection

Blobs are **never** garbage-collected in v1. A blob persists once
written, even if no live document state references it. A future GC
mechanism is anticipated; reimplementations **MAY** implement one but
**MUST NOT** assume the reference does so.

---

## 5. TLS material (hub only)

The hub stores its self-signed TLS keypair in a sibling of `.agentsync/`:

```
<vault-parent>/.agentsync-server/
├── tls.crt        ← X.509 cert in DER form
└── tls.key        ← ed25519 private key in PKCS#8 DER form, mode 0600
```

(The reference computes this directory as `<storage_path>/../.agentsync-server`
where `<storage_path>` is the `.agentsync/` directory.)

### 5.1 Generation

If `tls.crt` and `tls.key` both exist on hub start, they **MUST** be
loaded as-is.

If either is missing, the hub **MUST** generate a fresh self-signed
ed25519 keypair, valid for 10 years from the current wall-clock time,
and write both files atomically (write-tmp + rename) before serving any
connections. The reference uses `rcgen` for generation.

### 5.2 File modes

`tls.key` **MUST** be created with mode `0600` (owner read/write only)
on POSIX systems. A reimplementation on Windows **MAY** rely on
filesystem ACLs.

`tls.crt` has no mode requirement.

### 5.3 Cert content

The cert's Common Name, SAN list, and validity dates are not normatively
constrained beyond the 10-year lifetime SHOULD. Connecting peers
**MUST NOT** validate any of these (see [WIRE.md § 1.2](./WIRE.md#12-tls)).

---

## 6. `config.toml`

`.agentsync/config.toml` holds per-vault configuration. The format is
TOML.

### 6.1 Schema

```toml
[vault]
id              = "<UUID string>"            # optional locally; the doc carries the canonical vault_id
name            = "<display name>"           # optional
rendezvous_url  = "wss://hub.example:443"    # optional
hub_pubkey      = "ssh-ed25519 AAAA..."      # optional, set on TOFU

[identity]
path            = "<path to identity file>"  # optional; default is ~/.agentsync/id_ed25519
agent_socket    = "<path to ssh-agent socket>"  # optional, mutually exclusive with path
agent_pubkey    = "ssh-ed25519 AAAA..."      # optional, selects which key in the agent

[sync]
extensions             = ["md", "markdown"]   # which extensions to treat as text
include                = []                   # additional glob patterns (relative to vault root)
attachment_max_bytes   = 10485760             # 10 MiB
text_file_max_bytes    = 1048576              # 1 MiB
log_retention_days     = 30                   # reserved; not yet enforced
```

### 6.2 Defaults

| Field | Default if absent |
|---|---|
| `vault.id`              | (none — derived from `doc.bin`) |
| `vault.name`            | (none) |
| `vault.rendezvous_url`  | (none — vault is local-only) |
| `vault.hub_pubkey`      | (none — peer prompts on first connect) |
| `identity.path`         | `~/.agentsync/id_ed25519` |
| `identity.agent_socket` | (none — file-backed identity) |
| `identity.agent_pubkey` | (none) |
| `sync.extensions`       | `["md", "markdown"]` |
| `sync.include`          | `[]` |
| `sync.attachment_max_bytes` | `10485760` (10 MiB) |
| `sync.text_file_max_bytes`  | `1048576` (1 MiB) |
| `sync.log_retention_days`   | `30` |

A reimplementation **MUST** treat any absent section or field as its
default. Unknown fields **SHOULD** be ignored to permit forward-compat
extensions.

### 6.3 Mutual exclusion

`identity.path` and `identity.agent_socket` **MUST NOT** both be set
non-empty. If both are present, an implementation **MUST** error out
during config load.

### 6.4 Atomic write

The reference currently writes `config.toml` directly via `fs::write`,
which is *not* atomic. A reimplementation **SHOULD** use the
write-tmp + rename pattern for `config.toml` as well, especially for
TOFU updates (where `hub_pubkey` is appended after first connect).

---

## 7. `authorized_keys` (synced)

The vault's authorized peers list lives at `authorized_keys` *at the
root of the vault directory* (i.e., alongside the user's files), not
under `.agentsync/`. It is synced through the document like any other
text file. See [DOCUMENT.md § authorized_keys](./DOCUMENT.md#authorized-keys)
for its representation in the Automerge document.

### 7.1 Format

```
# agentsync authorized_keys
#
# One ssh-ed25519 public key per line.
# Lines starting with '#' are comments.

ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAA...  chris-macbook
ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAA...  chris-iphone
ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAA...  homelab-nas
```

### 7.2 Parser rules

A reimplementation **MUST** accept:

- Lines starting with `#` — comments, ignored.
- Empty lines — ignored.
- Lines of the form `ssh-ed25519 <base64> [label]` where:
  - `ssh-ed25519` is the literal key type. No other algorithms are
    supported in v1.
  - `<base64>` is the OpenSSH wire format of the public key (a 32-byte
    ed25519 public key wrapped in the standard SSH framing — see § 7.4
    below).
  - `label` is everything after `<base64>`, trimmed. May be empty.
- Leading whitespace **MUST** be trimmed before parsing.

A reimplementation **MUST** also accept the legacy bullet form
`- \`ssh-ed25519 ...\` — label` for backward compatibility with vaults
that used the deprecated `peers.md` format. Backticks and the leading
`- ` or `* ` **MUST** be stripped before normal parsing.

Unparseable lines **MUST** be silently skipped (not error). This is
because `authorized_keys` is also the user-facing UI for adding peers,
and a typo on one line should not lock everyone else out.

### 7.3 Render format

When agentsync writes `authorized_keys` programmatically, the output
**MUST** be:

```
# agentsync authorized_keys
#
# One ssh-ed25519 public key per line. Lines starting with '#' are
# comments. Paste `agentsync key show` output from any device you
# want to authorize.

<key1>
<key2>
...
```

Each key line is `ssh-ed25519 <base64>` or `ssh-ed25519 <base64> <label>`
if a label is set.

### 7.4 SSH wire format

The base64 portion of an `ssh-ed25519` line decodes to:

```
u32_be(11) || "ssh-ed25519" (11 bytes) || u32_be(32) || pubkey (32 bytes)
```

Total decoded length **MUST** be 51 bytes. A reimplementation **MUST**
validate this layout and the leading `"ssh-ed25519"` algorithm tag.

---

## 8. Identity files

### 8.1 Default location

The user's identity keypair is stored at:

- `${XDG_STATE_HOME or $HOME/.agentsync}/id_ed25519` — the private seed
- `${XDG_STATE_HOME or $HOME/.agentsync}/id_ed25519.pub` — the public key

The reference uses `~/.agentsync/id_ed25519` directly (not XDG-aware
yet). A reimplementation **MAY** follow the XDG Base Directory
specification.

The path **MAY** be overridden via:

- `--identity <path>` CLI flag,
- `AGENTSYNC_IDENTITY` environment variable, or
- the `[identity] path` field in `config.toml`.

### 8.2 Private file format

`id_ed25519` is a single-line text file:

```
agentsync-identity-v1 <base64-no-pad of 32-byte seed>
```

The leading literal `agentsync-identity-v1` is a domain tag. A
reimplementation **MUST** reject any file that does not begin with this
prefix.

The seed is the ed25519 32-byte secret seed (RFC 8032 § 5.1.5 input).
The implementation derives the keypair from this seed.

The file **MUST** be written with mode `0600` on POSIX. A line ending
(`\n`) **SHOULD** terminate the file.

### 8.3 Public file format

`id_ed25519.pub` is a single-line file in the OpenSSH `authorized_keys`
format:

```
ssh-ed25519 <base64 of SSH-wire encoding> [<comment>]
```

The comment is optional and **SHOULD** be omitted by default.

This file is informational; the reference regenerates it from the
private seed on demand. A reimplementation **MAY** omit it.

### 8.4 ssh-agent identities

When `[identity] agent_socket` is set, the implementation **MUST NOT**
read a private file. Instead, it connects to the agent over the
SSH-agent protocol, lists keys, and selects the key whose pubkey
matches `[identity] agent_pubkey`. If `agent_pubkey` is unset, the
implementation **MAY** default to the first ed25519 key advertised by
the agent.

ssh-agent details are governed by [draft-miller-ssh-agent][ssh-agent-draft].
Signing requests **MUST** carry the same 177-byte transcript bytes
described in [WIRE.md § 4.2](./WIRE.md#42-transcript).

[ssh-agent-draft]: https://datatracker.ietf.org/doc/html/draft-miller-ssh-agent

---

## 9. Atomic-write summary

A reimplementation **MUST** use atomic write (write-tmp + rename) for:

- `doc.bin`
- `snapshots/index.json`
- `blobs/<hash>` (with tmp name `.tmp.<hash>`)
- `tls.crt` and `tls.key`
- `id_ed25519` (with mode `0600` on POSIX)

A reimplementation **SHOULD** use atomic write for `config.toml`. The
reference does not currently do this; that is a known gap.

A reimplementation **MAY** call `fsync` before the rename for additional
crash durability. Whether to do so is an implementation choice; the
reference does not.

---

## 10. Conformance vectors

The following vectors live under `specs/vectors/storage/`:

- **`vectors/storage/doc-roundtrip.bin`** — a `doc.bin` produced by the
  reference for a known fixture.
- **`vectors/storage/index.json`** — a sample `snapshots/index.json`
  with a known set of labels.
- **`vectors/storage/config-minimal.toml`**, **`config-full.toml`** —
  example configs at the documented defaults and with every field set.
- **`vectors/storage/authorized_keys-cases.txt`** — a parser fixture
  containing well-formed lines, comments, blank lines, legacy bullet
  forms, and unparseable junk; paired with the expected parse output.

These are scaffolded in [vectors/README.md](./vectors/README.md).

---

## 11. Cross-references

- [DOCUMENT.md](./DOCUMENT.md) — the in-document representation of
  files (which `doc.bin` serializes), labels, and `authorized_keys`.
- [AUTH.md](./AUTH.md) — what `authorized_keys` and `hub_pubkey` mean
  semantically.
- [WIRE.md](./WIRE.md) — TLS material, channel binding, blob exchange.
- [HOST.md](./HOST.md) — the `DocStorage`, `BlobStorage`,
  `SnapshotStorage` traits a port must implement against this layout.
