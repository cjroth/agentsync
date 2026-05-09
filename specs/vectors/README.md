# Conformance Test Vectors

> Status: scaffolded. No vectors are checked in yet.
>
> The vector files described below are the planned set of golden test
> inputs. Generation lives in `tools/gen-vectors/` (also planned).

This directory holds the **conformance test vectors** for agentsync.
A reimplementation of any spec'd component (wire protocol, storage
format, document schema) can run its outputs against these vectors to
verify behavior matches the reference Rust implementation.

The vector set is the *only* normative way to verify a third-party
implementation interoperates byte-for-byte with the reference. The prose
specs ([WIRE.md](../WIRE.md), [STORAGE.md](../STORAGE.md),
[DOCUMENT.md](../DOCUMENT.md)) describe *what* to produce; the vectors
let you check that you produced it correctly.

## How vectors are organized

```
specs/vectors/
├── README.md                        ← this file
├── manifest.json                    ← machine-readable index
│
├── wire/
│   ├── transcript.bin               ← 177-byte transcript for a known fixture
│   ├── handshake.bin                ← 4-message handshake for the same fixture
│   ├── handshake-fixture.json       ← seeds, nonces, expected sigs
│   └── frame-encodings.json         ← every Frame variant + its msgpack hex
│
├── storage/
│   ├── doc-roundtrip.bin            ← reference doc.bin
│   ├── index.json                   ← reference snapshots/index.json
│   ├── config-minimal.toml          ← config with only required fields
│   ├── config-full.toml             ← config with every field set
│   ├── authorized_keys-cases.txt    ← parser fixture
│   └── authorized_keys-cases.json   ← expected parse output for each line
│
└── document/
    ├── empty.bin                    ← fresh Doc::new(vault_id)
    ├── empty.json                   ← expected schema_version, vault_id, …
    ├── with-files.bin               ← small set of text + attachment entries
    ├── with-files.json              ← expected list_files() output
    ├── with-labels.bin              ← one object-form, one legacy bytes-form label
    ├── with-labels.json             ← expected list_labels() output
    ├── with-deleted.bin             ← live + soft-deleted entries
    └── with-deleted.json            ← expected listing (deleted excluded)
```

## Manifest

`manifest.json` is the machine-readable index. Format:

```json
{
  "version": 1,
  "vectors": [
    {
      "id":    "wire/transcript-basic",
      "kind":  "wire-transcript",
      "input": "wire/handshake-fixture.json",
      "expect": "wire/transcript.bin",
      "spec":  "WIRE.md#42-transcript",
      "applies_to": ["all"]
    },
    {
      "id":    "storage/doc-roundtrip",
      "kind":  "automerge-roundtrip",
      "input": "storage/doc-roundtrip.bin",
      "expect": "storage/doc-roundtrip.bin",
      "spec":  "STORAGE.md#docbin",
      "applies_to": ["full"]
    }
  ]
}
```

`applies_to` lists which conformance profiles a vector applies to:

- `"all"`: every implementation, including read-only / browser-only.
- `"full"`: implementations that ship the full vault (read + write +
  network).
- `"hub"`: implementations that can run as `--listen`.

## How to run vectors against your implementation

Each vector is one of a small set of *kinds*. The kind determines
what your implementation does with the inputs and how it compares
against the expected output.

### `wire-transcript`

Given a `handshake-fixture.json` (hub seed, peer seed, hub nonce,
peer nonce, tls fingerprint), produce the 177-byte transcript per
[WIRE.md § 4.2](../WIRE.md#42-transcript) and compare bytes.

### `wire-handshake`

Given a fixture with private seeds, produce all four handshake
messages with the exact same nonces and expect byte-equal output.

### `wire-frame-roundtrip`

For each `Frame` value in `frame-encodings.json`:

1. Decode the hex string into bytes.
2. Parse via your MessagePack decoder.
3. Re-encode.
4. Verify the output equals the input.

### `storage-config-roundtrip`

Read `config-{minimal,full}.toml`, normalize defaults, write back.
Compare against the expected canonical form.

### `storage-authorized-keys-parse`

Parse `authorized_keys-cases.txt`. For each line, the expected output
appears in `authorized_keys-cases.json` (parsed peer or "skipped").

### `automerge-roundtrip`

Load the `.bin` via Automerge, save it back, and verify the resulting
bytes load to a document with identical state. Note: Automerge save
output is not byte-stable across versions, so this is *load-equivalence*
not byte-equality.

### `document-listing`

Load the `.bin`, call `list_files` / `list_labels` / `list_directories`,
compare against the expected JSON.

## How vectors are generated

The reference generator lives in `tools/gen-vectors/` (planned).
It is **deterministic**: given the fixed seeds and nonces in the
`*-fixture.json` files, the output bytes are reproducible. A
reimplementation that wants to contribute new vectors **MUST** be
deterministic in the same way.

Run:

```sh
cargo run --bin gen-vectors -- specs/vectors/
```

This regenerates every vector from its fixture. The CI pipeline runs
this and compares against the checked-in files; any drift is a
build failure.

## When vectors must be regenerated

Regenerate after any *intentional* change to:

- the wire protocol (handshake bytes, frame encoding, transcript
  layout),
- the document schema,
- the on-disk file formats.

Any change to those areas without a corresponding vector regeneration
is a build failure. This is deliberate — it forces the spec, the
code, and the vectors to stay aligned.

## Status

- **2026-05-09:** Directory scaffolded. No vectors checked in yet.
  The `tools/gen-vectors/` binary is planned. This README defines the
  shape so a reimplementation can plan against it.
