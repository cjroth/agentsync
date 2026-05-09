# agentsync

Real-time distributed agent memory: sync folders of markdown files between devices with point-in-time-recovery.

```sh
# Machine 1 — the hub
agentsync init
# Prints this device's pubkey, e.g.
#   identity_pub  = ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAA...
agentsync --listen
# Prints "listening on wss://0.0.0.0:443"
# 443 is privileged — see "Binding 443 as a regular user" below if you
# get "permission denied", or pick another port: `agentsync --listen 0.0.0.0:8443`.
```

```sh
# Machine 2 — a peer
agentsync key generate
# Generates a pubkey for this device at ~/.agentsync/id_ed25519.pub and prints it.
# Paste it into authorized_keys on Machine 1 (or any device that already has the vault)
# and let it sync.

agentsync clone machine-1
# Clones into a folder named after the remote vault's `name` (set on the
# hub via `agentsync init --name <name>`). Pass an explicit dir if you
# want to override: `agentsync clone machine-1 my-folder`. Bare hosts get
# `wss://` prepended (or `ws://` with `--no-tls`); pass `wss://host:port`
# / `ws://host:port` to be explicit.
# Port-less URLs use the scheme default (443 for wss, 80 for ws); include
# `:<port>` only if your hub binds something other than 443.
# On first connect you'll be prompted to confirm Machine 1's identity (TOFU).
# Pass --accept-hub-key <pubkey> to skip the prompt in scripts.
```

By default only `.md` and `.markdown` files sync; edit `[sync] extensions` in
`.agentsync/config.toml` to include other extensions.

* Dead-simple `agentsync` CLI that syncs between devices
* The CLI wraps a Rust SDK that can be imported to any Rust app
* Wasm support for TypeScript use cases is planned
* Built on Automerge which uses CRDTs to prevent merge conflicts
* Tag snapshots to easily go back to any point in time
* Per-device ed25519 identities; authorization via a synced `authorized_keys` file (SSH-style)
* Zero infrastructure required
* WSS with self-signed certs and channel-bound auth — no public CA needed
* ssh-agent backend supported for hardware-backed keys (Secretive, 1Password, ssh-agent, YubiKey-Agent)
* TOFU hub trust pinned per-vault in `config.toml`

**Status:** alpha. See [`SPEC.md`](./SPEC.md) for the product spec and [`AUTH.md`](./AUTH.md) for the auth design.

## Authentication model

Each device has its own ed25519 keypair. The hub (the `--listen` peer) gates
connections by checking the connecting peer's pubkey against `authorized_keys`,
an SSH-style file at the root of the synced vault that lists authorized devices:

```
# agentsync authorized_keys
ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAA... chris-macbook
ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAA... chris-iphone
ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAA... homelab-nas
```

`authorized_keys` is itself synced through agentsync, so removing a device
from any peer's copy disconnects them within one sync round. The line format
is identical to `~/.ssh/authorized_keys`, so you can paste OpenSSH pubkey
output directly.

By default the local identity lives at `~/.agentsync/id_ed25519` (shared
across all of this user's vaults, like `~/.ssh/id_ed25519`). Pass
`--identity <path>` on `init` / `clone` / `key generate` to override.

The wire is encrypted via WSS with a self-signed cert that the hub
auto-generates on first launch. Trust isn't established at the TLS layer —
clients accept any cert — but the application-layer signature in the
handshake binds to the cert fingerprint, so an active MITM that re-encrypts
to the real listener is detected and refused.

To add a new device:

1. On the new device, run `agentsync key generate` (or `agentsync init` for
   a fresh vault). Copy the printed `ssh-ed25519 ...` line.
2. On any device that already has the vault, append it as a new line in
   `authorized_keys`. The hub picks up the change via its file watcher
   and logs `peer added to authorized_keys`.
3. The new device can now connect.

To use a hardware-backed identity (Secretive, 1Password's ssh-agent,
gpg-agent, YubiKey-Agent), point agentsync at the agent socket:

```sh
agentsync watch \
  --identity-agent /path/to/agent.sock \
  --identity-agent-pubkey "ssh-ed25519 AAAA..."
```

Or persist the choice in `.agentsync/config.toml`:

```toml
[identity]
agent_socket = "/path/to/agent.sock"
agent_pubkey = "ssh-ed25519 AAAA..."
```

## Workspace layout

```
crates/
  agentsync-core/     # sync engine library
  agentsync-cli/      # `agentsync` binary
tests/
  e2e/                # multi-peer end-to-end tests against the real binary
SPEC.md               # product spec
AUTH.md               # auth design
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
| `agentsync init [--name NAME]` | Initialize a vault. Auto-generates a `name` from the directory basename (override with `--name`). Generates an ed25519 identity (default `~/.agentsync/id_ed25519`) and seeds `authorized_keys` with it. Also adds `.agentsync/` to `.gitignore` and `.agentsignore` and writes a starter `.syncignore` (gitignore-syntax exclude list for the sync engine) — skip all three with `--no-ignore-files`. |
| `agentsync watch [--authorized-keys KEYS]` | Watch and sync the vault at `--cwd` (default when no subcommand given). `--authorized-keys` (or the `AGENTSYNC_AUTHORIZED_KEYS` env var) merges extra `ssh-ed25519` lines into the synced `authorized_keys` on startup — handy for bootstrapping a server from a Fly.io / Railway secret. |
| `agentsync clone <url> [dir] [--vault-id ID] [--accept-hub-key PK] [--no-tls]` | Clone an existing vault. The local directory defaults to the remote vault's `name` (read from the handshake); pass `dir` to override. `--vault-id` is discovered via the handshake if omitted; `--accept-hub-key` skips the interactive TOFU prompt. URLs without a scheme are taken as `wss://` (or `ws://` with `--no-tls`). Port-less URLs use the scheme default (443 for wss, 80 for ws); include `:<port>` only when reaching a hub bound to something other than 443. |
| `agentsync status` | Print connection state, vault id, and local pubkey. |
| `agentsync push` / `pull` | One-shot sync. |
| `agentsync restore-at <when>` | Restore to a point in time. Accepts epoch ms or relative offsets like `5m`, `2h`, `1d`, `1w`. |
| `agentsync snapshot create/list/restore/delete` | Manage named recovery points. |
| `agentsync diff <from> [to]` | Show changes between two points in history. |
| `agentsync compact` | Run a compaction pass. |
| `agentsync key generate/show` | Generate this device's identity, or print its pubkey for pasting into someone else's `authorized_keys`. |
| `agentsync hub trust <pubkey>` / `forget` / `show` | Manage the pinned hub identity (`[vault] hub_pubkey`). |
| `agentsync completions <shell> [--install]` | Emit a shell-completion script (or `--install` to drop it in the conventional location for `bash`/`zsh`/`fish`). Supported shells: `bash`, `zsh`, `fish`, `powershell`, `elvish`. |

All subcommands operate on the vault at `--cwd` (or the `AGENTSYNC_CWD`
env var, falling back to the current working directory). Examples:

```sh
agentsync --cwd ~/notes status
AGENTSYNC_CWD=~/notes agentsync push
agentsync ~/notes        # bare-path shortcut → equivalent to `--cwd ~/notes watch`
```

The exceptions are `clone` (takes its own destination dir) and `init`
(creates the vault at `--cwd`).

### Letting a reverse proxy terminate TLS

If you're running the hub behind a managed proxy that already terminates TLS
(Railway, Render, Cloudflare Tunnel, an Nginx in front of you, …), pass
`--no-tls` to skip in-process TLS:

```sh
agentsync watch --listen --no-tls       # binds 0.0.0.0:80 (plain ws)
agentsync watch --listen 0.0.0.0:8080 --no-tls
```

Peers connect via the proxy's TLS endpoint — `agentsync clone wss://my-app.up.railway.app` —
or directly with `agentsync clone --no-tls my-app:8080` for a plain link on a
trusted network.

The same toggle is exposed as the `AGENTSYNC_NO_TLS` env var (set it to
`1` / `true` / `yes`), so on platforms like Railway you can flip it from
the dashboard without overriding the container start command:

```sh
# Railway (or any Docker-friendly host with edge TLS):
#   AGENTSYNC_NO_TLS=1
#   PORT=<whatever the platform injects>      # e.g. 8080
# The default Docker CMD picks both up automatically.
```

In `--no-tls` mode the cert-fingerprint channel binding is degraded: the hub
advertises an all-zero fingerprint, so MITM detection at the TLS layer is
delegated to the proxy. The hub identity (TOFU-pinned per vault) is still
verified end-to-end via the handshake signature, which a MITM cannot forge.
Use only when you trust the network path between the proxy and the hub
(public clouds typically run this hop on a private network).

### Binding 443 as a regular user

`agentsync --listen` binds `0.0.0.0:443` by default. On Unix, ports below
1024 require elevated privileges. Pick whichever fits your setup:

- **Linux** — grant the binary the bind capability once:

  ```sh
  sudo setcap cap_net_bind_service=+ep "$(which agentsync)"
  ```

  (Re-run after upgrades, since `setcap` xattrs don't survive a `cargo
  install` overwrite.)

- **macOS** — bind via a `LaunchDaemon` that runs as root and hands the
  socket off, or run the hub under `sudo`. There's no `setcap`
  equivalent.

- **Docker / Fly.io / Railway** — containers default to root, so binding
  443 inside them just works. No setup required.

- **Quick alternative** — pick an unprivileged port: `agentsync --listen
  0.0.0.0:8443`. Then peers clone with `wss://host:8443`.

### Tab completion

```sh
agentsync completions bash --install   # ~/.local/share/bash-completion/completions/agentsync
agentsync completions zsh  --install   # ~/.zfunc/_agentsync (add ~/.zfunc to $fpath)
agentsync completions fish --install   # ~/.config/fish/completions/agentsync.fish
```

For `powershell` / `elvish`, pipe stdout into your shell profile:

```powershell
agentsync completions powershell | Out-String | Invoke-Expression
```

`agentsync --help` for full flags.

## On-disk layout

agentsync state lives next to your files in `.agentsync/`:

```
my-vault/
├── notes/                       ← your files, plain on disk
├── authorized_keys              ← authorized device pubkeys (synced, SSH-style)
├── README.md
├── .gitignore                   ← seeded by `init` to ignore .agentsync/
├── .agentsignore                ← same, for agentsync's own ingest filter
├── .syncignore                  ← gitignore-syntax exclude list (per-vault sync engine)
├── .agentsync/                  ← per-vault state, managed by the CLI
│   ├── config.toml              ← vault id, name, rendezvous url, identity path, hub_pubkey
│   ├── doc.bin                  ← saved Automerge document (full history)
│   ├── snapshots/index.json     ← named labels → heads
│   └── blobs/<sha256>           ← binary attachments
└── .agentsync-server/           ← only on a `--listen` peer
    ├── tls.crt                  ← self-signed cert (10-year, ed25519)
    └── tls.key                  ← private key for the cert (mode 0600)

~/.agentsync/                    ← shared across this user's vaults
├── id_ed25519                   ← ed25519 secret seed (mode 0600)
└── id_ed25519.pub               ← matching ssh-ed25519 pubkey
```

Back up `.agentsync/` with any tool you like (restic, borgbackup, rclone) — it
contains the full document history. `.agentsync-server/` only matters if this
device runs `--listen`; deleting it just regenerates the cert (existing peers
will need to re-pin the new fingerprint).

## Running as a hub (Docker / Fly.io / Railway)

The included `Dockerfile` builds a tiny image that runs `agentsync watch
--listen` on a persisted volume. Two env vars control bootstrap:

- `AGENTSYNC_VAULT_NAME` — name written into `config.toml` on first launch.
  Used as the default local directory when peers run
  `agentsync clone wss://your-host`.
- `AGENTSYNC_AUTHORIZED_KEYS` — `ssh-ed25519 <base64> [comment]` entries
  separated by newlines or commas. Merged into the synced `authorized_keys` on every `watch`
  startup; existing keys are skipped, so it's safe to leave set across
  restarts.

Fly.io example (see `fly.toml`):

```sh
fly launch
fly secrets set AGENTSYNC_AUTHORIZED_KEYS="$(cat ~/.agentsync/id_ed25519.pub)"
# After first deploy, pin the hub's identity locally:
fly logs | grep identity_pub
agentsync clone wss://your-app.fly.dev --accept-hub-key "ssh-ed25519 ..."
```

## Disaster recovery

### I deleted my local clone and now files are missing on the hub

If you `rm -rf` a clone while `agentsync watch` (or an active `agentsync
clone`) is still running, the deletions of your *data files* propagate to
every peer through the live sync session — there's no separate confirmation
step. `authorized_keys` is the one exception: a delete event for it is
dropped at the FS-event layer, so you can't accidentally lock everyone out
by wiping the directory. To recover the data files:

1. Re-clone the vault (`agentsync clone <url>`). You'll get the deleted
   state — empty or near-empty.
2. Rewind the doc to before the deletion: `agentsync restore-at 5m` (or
   whatever offset / epoch-ms target you need). `restore-at` is *additive*:
   it appends new forward-going changes that re-create the files, rather
   than rewriting history, so the recovery itself syncs back to the hub
   cleanly.
3. Run `agentsync watch` to push the un-delete changes up. The hub and any
   other peers will converge on the restored state.

To avoid the situation entirely, **stop the watcher before deleting a
clone** — Ctrl-C the running `agentsync watch` / `agentsync clone`, then
`rm -rf`.

### My hub is on Fly / Railway and the operator key got removed

If the operator's bootstrap pubkey ends up missing from `authorized_keys`
(e.g. someone hand-edited the file or the file predates the delete guard),
restart the hub container. On boot, `agentsync watch` re-merges
`AGENTSYNC_AUTHORIZED_KEYS` into the synced doc — so as long as the secret
is still set, the bootstrap key comes back. Then re-clone locally and (if
needed) `agentsync restore-at` to roll back any other lost state.

## Testing

```bash
cargo test --workspace            # everything (unit + e2e)
cargo test --lib                  # unit tests only
cargo test -p agentsync-e2e       # multi-peer end-to-end tests only
```

E2E tests spawn the real `agentsync` binary in temp directories and exercise
sync over real WSS connections, including the four-message handshake, the
ssh-agent signing path (via an in-process mock agent), and an active-MITM
relay test that verifies channel binding refuses tampered connections. Per
the spec: if a feature isn't covered by an E2E test, it doesn't ship.

## License

Not yet licensed - I haven't decided yet.
