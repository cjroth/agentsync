# agentsync

Real-time distributed agent memory: sync folders of markdown files between devices with point-in-time-recovery.

```sh
# Machine 1
agentsync init
# Prints `vault_key = <base64>` — copy that.
export AGENTSYNC_KEY="<key-from-init>"
agentsync --listen 0.0.0.0:1234
```

```sh
# Machine 2
export AGENTSYNC_KEY="<same-key-from-machine-1>"
agentsync clone cloned-folder --rendezvous ws://machine-1:1234
```

By default only `.md` and `.markdown` files sync; edit `[sync] extensions` in
`.agentsync/config.toml` to include other extensions.

* Dead-simple `agentsync` CLI that syncs between devices
* The CLI wraps a Rust SDK that can be imported to any Rust app
* Wasm support for TypeScript use cases is planned
* Built on Automerge which uses CRDTs to prevent merge conflicts
* Tag snapshots to easily go back to any point in time
* Auth is a single 32-byte shared key (base64-encoded)
* Zero infrastructure required
* Plaintext `ws://` only in this build — TLS via a fronting proxy or v2

**Status:** alpha. See [`SPEC.md`](./SPEC.md) for the full product spec.

## Workspace layout

```
crates/
  agentsync-core/     # sync engine library
  agentsync-cli/      # `agentsync` binary
tests/
  e2e/                # multi-peer end-to-end tests against the real binary
SPEC.md               # product spec
```

## Build

```bash
cargo build --release
./target/release/agentsync --version
```

Requires Rust 1.89+.

## CLI commands

| Command | Description |
| --- | --- |
| `agentsync init` | Initialize a vault in the current directory. |
| `agentsync watch [path]` | Watch and sync a directory (default when no subcommand given). |
| `agentsync clone <path> --rendezvous URL [--vault-id ID]` | Clone an existing vault to a local directory. `--vault-id` is optional; the server returns it during the handshake if omitted. |
| `agentsync status` | Print connection state and counts. |
| `agentsync push` / `pull` | One-shot sync. |
| `agentsync restore-at <timestamp>` | Restore the vault to a wall-clock moment (RFC3339 or epoch ms). |
| `agentsync snapshot create/list/restore/delete` | Manage named recovery points. |
| `agentsync diff <from> [to]` | Show changes between two points in history. |
| `agentsync compact` | Run a compaction pass. |
| `agentsync key generate/show/store` | Manage vault keys. |

`agentsync --help` for full flags.

## On-disk layout

agentsync state lives next to your files in `.agentsync/`:

```
my-vault/
├── notes/                       ← your files, plain on disk
├── README.md
└── .agentsync/                  ← managed by the CLI
    ├── config.toml              ← vault id, rendezvous url, key source
    ├── doc.bin                  ← saved Automerge document (full history)
    ├── snapshots/index.json     ← named labels → heads
    ├── blobs/<sha256>           ← binary attachments
```

Back up `.agentsync/` with any tool you like (restic, borgbackup, rclone) — it contains the full document history.

## Testing

```bash
cargo test --workspace            # everything (unit + e2e)
cargo test --lib                  # unit tests only
cargo test -p agentsync-e2e       # multi-peer end-to-end tests only
```

E2E tests spawn the real `agentsync` binary in temp directories and exercise
sync over real WebSocket connections. Per the spec: if a feature isn't
covered by an E2E test, it doesn't ship.

## License

Not yet licensed - I haven't decided yet.
