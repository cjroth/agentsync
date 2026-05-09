# DOCUMENT.md — Automerge Document Schema

> Normative spec. See [SPEC.md § Conformance language](./SPEC.md#conformance-language)
> for RFC 2119 keyword usage.

This document specifies the logical schema of the single Automerge
document that is the source of truth for a vault. The on-disk byte
encoding of the document is Automerge's native columnar format and is
out of scope for this spec — see [STORAGE.md § doc.bin](./STORAGE.md#docbin)
for persistence rules.

---

## 1. Top-level keys

The Automerge document's root **MUST** be an Automerge map. It **MUST**
have these keys, with these types, and **MUST NOT** have any other
top-level keys (a reimplementation **SHOULD** ignore unknown keys for
forward compatibility, but **MUST NOT** create them).

| Key | Automerge type | Cardinality | Purpose |
|---|---|---|---|
| `schema_version` | scalar `int` | exactly one | always `1` for this spec |
| `vault_id`       | scalar `str` | exactly one | UUID identifying this vault |
| `directories`    | map         | exactly one | directory entries keyed by UUID |
| `files`          | map         | exactly one | file entries keyed by UUID |
| `labels`         | map         | exactly one | named recovery points keyed by label name |

### 1.1 `schema_version`

`schema_version` **MUST** equal `1`. A reimplementation that loads a
document with a different value **MUST** error out and **MUST NOT**
attempt to interpret the rest of the document.

### 1.2 `vault_id`

A UUID (canonical lowercase 8-4-4-4-12 form) chosen at vault creation.
A reimplementation **MUST NOT** mutate `vault_id` after creation.

When a vault is opened, the implementation **SHOULD** verify that
`vault_id` in the loaded document matches the `vault_id` in the local
`config.toml` (if set). Mismatch indicates a vault swap and **SHOULD**
error.

### 1.3 `directories`, `files`, `labels`

Each is an Automerge map. `directories` and `files` are keyed by UUID;
`labels` is keyed by the label name. Their value schemas are specified
below.

### 1.4 Genesis actor

When creating a new document, the implementation **SHOULD** set the
Automerge actor ID to a stable value derived from `vault_id`. The
reference uses a SHA-256-based derivation so all peers initialising the
same vault produce comparable history.

This is not strictly required for correctness — Automerge merges
regardless of actor — but using a deterministic genesis actor avoids
spurious history-divergence on multi-peer fresh setups.

---

## 2. Files

### 2.1 Map shape

`root.files` is a flat Automerge map. Each entry's *key* is a freshly
generated UUID (the `FileId`). Each entry's *value* is an Automerge map
with the fields specified below.

A reimplementation **MUST NOT** key `files` by file path. Path is
metadata; it changes on rename. The UUID key is the stable identity.

### 2.2 File entry fields

```
files["<UUID>"] = {
    "meta":         <map: FileMeta>,
    "content":      <Automerge Text>,    // text files only
    "binary_hash":  <scalar str>,        // attachments only
}
```

`meta` is **required** for every file entry.

Exactly one of `content` or `binary_hash` **MUST** be present, depending
on `meta.kind`:

- `meta.kind == "text"` → `content` MUST be an Automerge Text object;
  `binary_hash` MUST be absent.
- `meta.kind == "attachment"` → `binary_hash` MUST be a scalar string;
  `content` MUST be absent.

### 2.3 `meta` field schema

```
files["<UUID>"]["meta"] = {
    "id":          <scalar str>,    // UUID, equal to the parent map key
    "path":        <scalar str>,    // POSIX-normalized path; see § 5
    "kind":        <scalar str>,    // "text" or "attachment"
    "size":        <scalar int>,    // bytes
    "created_at":  <scalar int>,    // ms since Unix epoch
    "updated_at":  <scalar int>,    // ms since Unix epoch
    "deleted_at":  <scalar int>?,   // optional; ms since Unix epoch
    "binary_hash": <scalar str>?,   // optional; mirror of entry.binary_hash for attachments
}
```

Fields:

- `id` **MUST** equal the UUID used as the parent map key. (This
  duplication is for convenience — a reimplementation **MUST** keep them
  in sync.)
- `path` **MUST** be a non-empty POSIX path normalised per § 5.
- `kind` **MUST** be exactly the string `"text"` or `"attachment"`.
- `size` is the file's byte length. For text files it is the UTF-8 byte
  length of the Text content; for attachments it is the size of the
  blob.
- `created_at`, `updated_at` are wall-clock millisecond timestamps. They
  are advisory — Automerge's own change graph is the authoritative
  ordering — but they MUST be monotonic per file (i.e., `updated_at >=
  created_at`).
- `deleted_at` indicates a soft-deleted file. When present, the entry
  **MUST** be excluded from `list_files()` results but **MUST** remain
  in the map (so renames and history are preserved). When absent (or
  null), the file is live.
- `binary_hash` (in `meta`) is only set for attachments. It is
  redundant with the entry's top-level `binary_hash` field; the two
  **MUST** be kept in sync if both are written.

### 2.4 Text body

For `kind == "text"`, the file's content is an Automerge `Text`
object at `files["<UUID>"]["content"]`. The Text's full string contents
are the body of the file.

A reimplementation **MUST** edit the body via Automerge `splice_text`
(or equivalent) operations rather than wholesale replacement, so
concurrent edits CRDT-merge. A wholesale replacement is a valid Text
operation but defeats merge.

### 2.5 Attachment body

For `kind == "attachment"`, `files["<UUID>"]["binary_hash"]` is a
scalar string equal to the lowercase hexadecimal SHA-256 of the
attachment's bytes. The actual bytes live in
`.agentsync/blobs/<hash>` (see [STORAGE.md § Blob store](./STORAGE.md#blob-store))
and are exchanged via [WIRE.md § 7](./WIRE.md#7-blob-exchange).

A reimplementation **MUST NOT** store attachment bytes inside the
Automerge document. The blob hash is the only document-side reference.

### 2.6 Lookup by path

A reimplementation **MUST** implement "find file by path" as a linear
scan over the live (non-deleted) entries comparing `meta.path` to the
normalized query path. There is no path index in v1.

If two live entries share the same `path`, an implementation **MAY**
return either one (concurrent creates can produce this; CRDT semantics
do not enforce path uniqueness). A reimplementation **MAY** flag the
duplicate to the user.

---

## 3. Directories

### 3.1 Map shape

`root.directories` mirrors `root.files`: a flat map keyed by UUID, each
value an Automerge map carrying directory metadata.

```
directories["<UUID>"] = {
    "id":          <scalar str>,    // UUID, equal to parent map key
    "path":        <scalar str>,    // POSIX-normalized path
    "created_at":  <scalar int>,    // ms since Unix epoch
    "deleted_at":  <scalar int>?,   // optional soft-delete marker
}
```

### 3.2 Implicit vs explicit directories

A directory entry **MUST** exist in `directories` if any of the
following is true:

- the user explicitly created it (e.g., to model an empty directory),
- it is the parent of a live file or directory, *and* an implementation
  is configured to materialize parent dirs explicitly.

The reference creates ancestor directory entries lazily when files are
written. A reimplementation **MAY** keep `directories` empty unless the
user explicitly creates one — but on filesystem materialization it
**MUST** create any missing ancestor directories on disk regardless.

A directory entry's absence does not mean the directory does not exist
on disk; a directory always exists if any live file lists it as an
ancestor.

### 3.3 Recursive delete

`delete_directory(path, recursive=true)` **MUST** be a single Automerge
transaction that soft-deletes:

- the directory entry, and
- every descendant file entry, and
- every descendant directory entry.

This atomicity is a feature of one-Automerge-doc-per-vault; a
reimplementation **MUST** preserve it (otherwise concurrent peers may
observe inconsistent partial deletion states).

---

## 4. Labels

### 4.1 Map shape

`root.labels` is an Automerge map keyed by label name. Each value is
either:

- **(canonical, v1)** an Automerge map: the *object form*, or
- **(legacy)** a scalar `bytes`: the *bytes form*.

A reimplementation **MUST** read both. It **MUST** write only the object
form.

### 4.2 Object form

```
labels["<label-name>"] = {
    "heads":       <scalar bytes>,    // see § 4.4 for encoding
    "created_at":  <scalar int>,      // ms since Unix epoch
}
```

`heads` is required. `created_at` is required for new labels; readers
**MUST** treat absence as `0` (epoch) for forward compatibility.

### 4.3 Legacy bytes form

A label whose value is a scalar `bytes` directly (no enclosing object)
**MUST** be interpreted as `heads` only, with `created_at` defaulting
to `0`. This form is preserved for backward compat with vaults created
by earlier reference versions.

A reimplementation **SHOULD NOT** create new labels in the legacy form.
Whether to *upgrade* legacy labels in place to the object form is an
implementation choice; the reference does not.

### 4.4 `heads` encoding

The `heads` value encodes a set of Automerge `ChangeHash` values, each
of which is exactly 32 bytes. The encoding is **byte concatenation**:

```
heads = changeHash[0] || changeHash[1] || ... || changeHash[N-1]
```

A reimplementation **MUST** reject a `heads` value whose length is not
a multiple of 32. The order of hashes is *not* significant — Automerge
treats heads as a set — but readers **SHOULD** preserve order on
round-trip.

The same encoding is used for the on-disk `snapshots/index.json` cache
([STORAGE.md § 3](./STORAGE.md#snapshotsindexjson)), where the bytes
are then base64-encoded for JSON.

### 4.5 Label invariants

- Label names are arbitrary UTF-8 strings. There is no length limit in
  v1, but a reimplementation **SHOULD** reject names containing control
  characters or excessive length (>= 1024 bytes).
- Two peers concurrently creating labels with the same name CRDT-merge
  to one of the two values; this is acceptable. Concurrent
  delete-create on the same name is also CRDT-resolved.

### 4.6 Sync to local cache

The on-disk `snapshots/index.json` is a derived view of `root.labels`.
After every label-affecting operation (create, delete, restore), the
implementation **SHOULD** rewrite the index from the document. The
reference does this synchronously.

---

## 5. Path normalization

All `path` fields in the document **MUST** be POSIX-normalized.
Specifically:

- Use forward slashes only. On Windows, the implementation **MUST**
  translate backslashes before storage.
- No leading slash. Paths are *vault-relative*, e.g., `"notes/todo.md"`,
  not `"/notes/todo.md"`.
- No `.` or `..` segments.
- No empty segments (no `"a//b"`).
- Unicode strings **MUST** be normalised to NFC.
- Paths are case-sensitive on the wire and in the document. (Local
  filesystem case-folding is the materialization layer's concern.)

A reimplementation **MUST** validate paths at ingest (any API that
takes a path) and reject malformed inputs with a clear error. It
**MUST NOT** silently coerce.

The reference's `path::normalize` function is the canonical implementation.

---

## 6. `authorized_keys`

`authorized_keys` is **a regular file** in the document — not a special
top-level key. It lives at:

```
files["<UUID-of-authorized_keys-entry>"] = {
    "meta":     { "path": "authorized_keys", "kind": "text", ... },
    "content":  <Automerge Text>,
}
```

The format of the Text body is specified in
[STORAGE.md § authorized_keys](./STORAGE.md#authorized_keys-synced).

A reimplementation that needs to enumerate authorized peers **MUST**:

1. Look up the file entry whose `meta.path == "authorized_keys"`.
2. Read its `content` Text as a string.
3. Parse using the rules from STORAGE.md § 7.

A reimplementation **MUST NOT** invent an alternative location for the
authorized list. The choice of "synced through the document, like any
other file" is foundational to the trust model — see
[AUTH.md](./AUTH.md).

A vault **SHOULD** be initialized with `authorized_keys` containing
the creator's pubkey, so the creator can connect immediately. The
reference does this in `Vault::create`.

---

## 7. Soft deletes

Both `files` and `directories` use a `deleted_at` timestamp for
soft-delete. Hard-deleting an entry from the map is **forbidden** in v1
because:

- Renames are tracked via stable UUIDs; deleting an entry breaks
  history reconstruction.
- Point-in-time recovery to a moment when the file was live must still
  return its content.
- Concurrent recreation at the same path must be distinguishable from
  the original.

Visibility rules:

- `list_files()` **MUST** filter out entries where `deleted_at` is
  present (non-null).
- `read_file(path)` **MUST** treat a deleted entry as not-found.
- `restore_to_heads(...)` may produce a state where an entry's
  `deleted_at` is unset; the listing then becomes visible again.

Filesystem materialization **MUST** delete the on-disk file when an
entry transitions from live to soft-deleted, and create it when the
reverse happens.

---

## 8. ID generation

All UUIDs in the document (`FileId`, `DirId`) **MUST** be generated by
a process that produces values which are unique with overwhelming
probability across all peers. The reference uses UUID v4
(122 random bits via OS RNG).

A reimplementation **MUST NOT** use sequential or path-derived IDs;
that breaks rename semantics and CRDT identity.

---

## 9. Schema invariants (summary)

A document is *well-formed* if all of the following hold at every
moment:

1. `root.schema_version == 1`.
2. `root.vault_id` is a non-empty string and stable across all changes.
3. `root.directories`, `root.files`, `root.labels` are all map objects.
4. Every entry in `root.files` has a `meta` map with `id`, `path`,
   `kind`, `size`, `created_at`, `updated_at`.
5. For every file entry, exactly one of `content` or `binary_hash`
   is present (consistent with `meta.kind`).
6. Every entry in `root.directories` has `id`, `path`, `created_at`.
7. Every label value is either an object with `heads` or a `bytes`
   scalar (legacy).
8. Every `path` is POSIX-normalized per § 5.

A reimplementation **MAY** validate these invariants on load and
**SHOULD** report violations as a corrupt-document error rather than
silently coercing.

CRDT merges **MAY** transiently create states that violate uniqueness
expectations (e.g., two live files at the same path). These are not
spec violations — they are valid CRDT outcomes the user must resolve.

---

## 10. Conformance vectors

The following vectors live under `specs/vectors/document/`:

- **`vectors/document/empty.bin`** — a fresh document immediately after
  `Doc::new(vault_id)` with no files, directories, or labels.
- **`vectors/document/with-files.bin`** — a document with a small set
  of text files and one attachment, paired with a JSON manifest naming
  the expected `list_files()` output.
- **`vectors/document/with-labels.bin`** — a document with one
  object-form label and one legacy bytes-form label, plus expected
  `list_labels()` output.
- **`vectors/document/with-deleted.bin`** — a document with both live
  and soft-deleted entries, plus expected listing output.

These are scaffolded in [vectors/README.md](./vectors/README.md).

---

## 11. Cross-references

- [STORAGE.md](./STORAGE.md) — how the document persists to disk and
  how `authorized_keys` parses.
- [WIRE.md](./WIRE.md) — sync protocol that propagates document
  changes; blob exchange for attachment bodies.
- [AUTH.md](./AUTH.md) — semantic interpretation of `authorized_keys`.
- [API-RUST.md](./API-RUST.md), [API-TS.md](./API-TS.md) — the
  programmatic interfaces that read and write this schema.
