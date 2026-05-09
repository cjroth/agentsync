# AUTH.md — Authentication and Authorization

> Normative spec. See [SPEC.md § Conformance language](./SPEC.md#conformance-language)
> for RFC 2119 keyword usage.

This document specifies the trust model: who can connect to a vault,
how the connection is authenticated, and what an attacker is and is
not prevented from doing. It cross-references [WIRE.md](./WIRE.md) for
byte-level handshake details and [STORAGE.md](./STORAGE.md) for the
file formats involved.

---

## 1. Model

agentsync's authentication model is SSH-shaped:

- Each device has its own ed25519 identity keypair. There is no
  shared secret across the vault.
- The vault has an `authorized_keys` file at its root listing the
  public keys allowed to connect. This file is itself synced through
  the vault.
- One peer in the vault runs as the *hub* (the peer with `--listen`).
  The hub is a normal vault participant that happens to listen — it
  has full plaintext access to the vault.
- Connecting peers pin the hub's identity pubkey on first connect
  (TOFU), stored in their local `config.toml`. Subsequent mismatches
  are refused.

There is **no** concept of users, accounts, roles, or per-file
permissions in v1. Possession of an authorized device key grants full
read/write access to the entire vault.

There is **no** end-to-end encryption above TLS. The hub holds vault
contents in plaintext on its filesystem. The hub **MUST** be a
trusted machine the operator controls.

---

## 2. Identity keys

### 2.1 Algorithm

All identity keys are **ed25519** (RFC 8032). No other algorithms are
supported in v1. A reimplementation that allows other algorithms is
non-conformant.

| Constant | Value |
|---|---|
| `PUBKEY_LEN`     | 32 bytes |
| `SIGNATURE_LEN`  | 64 bytes |
| Seed length      | 32 bytes |

### 2.2 Storage

The default storage location is `~/.agentsync/id_ed25519` with
`0600` mode. The on-disk format is:

```
agentsync-identity-v1 <base64-no-pad of 32-byte seed>
```

See [STORAGE.md § 8](./STORAGE.md#identity-files) for the full file
format and override paths.

A reimplementation **MAY** also support agent-backed identities (see
§ 2.4).

### 2.3 Wire format

When a public key appears in `authorized_keys` or in `hub_pubkey`, it
is in OpenSSH's `authorized_keys` line format:

```
ssh-ed25519 <base64> [<comment>]
```

The base64 portion decodes to the SSH wire encoding:

```
u32_be(11) || "ssh-ed25519" (11 bytes) || u32_be(32) || pubkey (32 bytes)
```

(Total: 51 bytes.) See [STORAGE.md § 7.4](./STORAGE.md#ssh-wire-format)
for the parser contract.

### 2.4 Agent-backed identities

A peer **MAY** delegate identity signing to an external agent over
the SSH-agent protocol ([draft-miller-ssh-agent][ssh-agent-draft]).
This supports hardware-backed identities (Secretive, 1Password,
gpg-agent, YubiKey-Agent, OpenSSH `ssh-agent`).

[ssh-agent-draft]: https://datatracker.ietf.org/doc/html/draft-miller-ssh-agent

Configuration:

```toml
[identity]
agent_socket = "/path/to/agent.sock"
agent_pubkey = "ssh-ed25519 AAAA..."   # selects which key in the agent
```

Discovery order:

1. `--identity-agent <path>` flag.
2. `[identity] agent_socket` in `config.toml`.
3. `$SSH_AUTH_SOCK` environment variable.

If multiple keys are advertised by the agent, `agent_pubkey` selects
which one to use. If unset, the implementation **MAY** default to the
first ed25519 key in the agent's identity list.

The agent **MUST** be asked to sign the same 177-byte transcript
defined in [WIRE.md § 4.2](./WIRE.md#42-transcript). A reimplementation
**MUST NOT** modify the transcript before sending to the agent.

---

## 3. Handshake

The complete handshake is specified in
[WIRE.md § 4](./WIRE.md#handshake-normative). This section names the
properties it provides and the failure modes a reimplementation
**MUST** handle.

### 3.1 Mutual authentication

Both sides sign and verify the transcript. After a successful
handshake, each side has cryptographic proof that:

- the other side controls the corresponding ed25519 private key, and
- the same TLS channel was observed by both (channel binding —
  see [WIRE.md § 4.5](./WIRE.md#45-channel-binding)).

### 3.2 Authorization (hub side)

After receiving `HelloPeer`, the hub **MUST** look up the connecting
peer's `peer_identity_pubkey` in the vault's `authorized_keys` file. If
absent, the hub **MUST**:

1. Send `Frame::Error{message}` identifying the rejection reason.
2. Close the WebSocket cleanly.
3. **MUST NOT** send `ProofHub` or any subsequent frame.

This means an unauthorized peer never sees a hub signature. The hub's
public identity is therefore not leaked to unauthorized scanners
beyond the cert (which is visible at TLS time anyway).

### 3.3 Hub trust (peer side)

The connecting peer pins the hub's identity in its local
`.agentsync/config.toml`:

```toml
[vault]
hub_pubkey = "ssh-ed25519 AAAA..."
```

Behavior:

- **Unset on connect:** the peer **SHOULD** prompt the operator with
  the presented hub fingerprint and persist the answer.
- **Match:** the connection proceeds silently.
- **Mismatch:** the connection **MUST** be aborted before any sync
  data flows, and a clear warning **MUST** be presented identifying
  both keys.

The TOFU prompt (reference UX):

```
The hub at wss://hub.example.com:443 has identity:
  ssh-ed25519 AAAA...xyz
  SHA256:7p/Q3F...

This is the first time connecting. Trust this hub? [y/N]
```

The mismatch warning:

```
WARNING: HUB IDENTITY HAS CHANGED!
Stored:    ssh-ed25519 AAAA...xyz
Presented: ssh-ed25519 AAAA...abc

Either the hub's identity key was rotated, or someone is
impersonating it. Refusing to connect. To accept the new key, run:
  agentsync hub trust ssh-ed25519 AAAA...abc
```

A reimplementation **MAY** offer:

- `--accept-hub-key <pubkey>` for non-interactive setups (equivalent
  to pre-populating `hub_pubkey`).
- An out-of-band mechanism to clear the pin (the reference uses
  `agentsync hub forget`).

### 3.4 Channel binding

If the peer's transport can observe the hub's TLS certificate, it
**MUST** verify that the SHA-256 of the cert DER matches the
`tls_cert_fingerprint` advertised in `HelloHub` and signed in the
transcript. Mismatch indicates a relayed MITM; the peer **MUST**
abort.

Browser `WebSocket` cannot observe the cert. In that case, the hub
**MUST** still advertise the fingerprint (so non-browser peers can
verify), and the browser peer **MUST** treat its missing
channel-binding capability as degraded mode and **SHOULD** warn.

### 3.5 Cipher agility / version

There is no in-band protocol negotiation. The 17-byte transcript
prefix `agentsync-auth-v1` is a domain-separation tag. Future
protocol changes bump the tag (`agentsync-auth-v2`) and constitute a
coordinated break — old and new clients cannot interoperate.

A reimplementation **MUST NOT** introduce a version-negotiation
phase without coordinating with the spec.

---

## 4. `authorized_keys`

### 4.1 Where it lives

`authorized_keys` is a regular text file at the root of the synced
vault. It is stored inside the Automerge document like any other
text file (see [DOCUMENT.md § 6](./DOCUMENT.md#authorized_keys)) and
materialized to disk as `<vault>/authorized_keys`.

A reimplementation **MUST NOT** invent a different location.

### 4.2 Format

See [STORAGE.md § 7](./STORAGE.md#authorized_keys-synced) for the
parser and renderer contract. Summary:

- One `ssh-ed25519 <base64> [label]` line per authorized key.
- `#`-prefixed lines are comments.
- Blank lines are ignored.
- Legacy `- \`ssh-ed25519 ...\`` bullet lines are accepted (read-only).
- Unparseable lines are silently skipped — the file is also a UI for
  humans to edit, and a single typo must not lock everyone out.

### 4.3 Mutation

A reimplementation **MAY** offer programmatic helpers
(`agentsync key add`, etc.). At the protocol level, all that matters
is that the file's contents change in the synced document.

The hub **MUST** re-read `authorized_keys` whenever the document
changes and disconnect any currently-connected peer no longer
authorized. The reference implementation does this on every
doc-change notification. Removal is therefore eventually consistent —
see § 6.2.

### 4.4 Bootstrap

A new vault **SHOULD** be initialized with `authorized_keys`
containing the creator's pubkey, so the creator can immediately
connect from the same device they used to initialize. The reference
does this in `Vault::create`.

To add a second device:

1. On the new device, generate an identity (`agentsync key generate`
   or `agentsync init` for a fresh vault).
2. On any device that already has the vault, append the new device's
   pubkey as a line in `authorized_keys`.
3. The hub picks up the change (file watcher), re-parses, and the new
   device can connect.

There is no online "request access" flow in v1. Adding a peer is
strictly out-of-band.

---

## 5. TLS

### 5.1 Self-signed certs

The hub generates a self-signed ed25519 X.509 certificate on first
run and persists it under `<vault-parent>/.agentsync-server/`
(see [STORAGE.md § 5](./STORAGE.md#tls-material-hub-only)). Connecting
clients **MUST NOT** validate hostname, expiry, or chain — TLS trust
is delegated to the application-layer channel binding.

This means agentsync works on hostname-less LAN deployments and behind
NATs without ACME / Let's Encrypt / public CAs.

### 5.2 Reverse-proxy TLS termination

A hub running behind a TLS-terminating reverse proxy (Fly.io,
Railway, Caddy) **MUST** be started with `--no-tls` (or equivalent),
which:

- binds plain `ws://`, expecting the proxy to terminate TLS,
- emits a 32-byte zero `tls_cert_fingerprint` in `HelloHub`,
- puts every connection in degraded channel-binding mode.

In this configuration:

- The reverse proxy **MUST** be configured to refuse non-TLS inbound
  traffic.
- Clients **SHOULD** still connect with `wss://` URLs (so the proxy
  hop is encrypted).
- Channel binding is unavailable — peers rely solely on TLS
  validation against the proxy's cert (handled by the underlying
  TLS stack) plus identity-pubkey pinning.

### 5.3 Cert rotation

There is no automated cert rotation in v1. To rotate:

1. Stop the hub.
2. Delete `<vault-parent>/.agentsync-server/tls.crt` and `tls.key`.
3. Restart the hub. A fresh cert is generated.

Existing connecting peers **MAY** see channel-binding mismatches on
reconnect; in degraded-binding mode (browser peer / `--no-tls` hub)
the rotation is invisible to clients.

---

## 6. Threat model

### 6.1 Defended

| Attacker | How |
|---|---|
| Passive eavesdropper on the network | TLS 1.3 confidentiality + forward secrecy |
| Active MITM relaying the handshake | Channel binding (signature covers TLS cert fingerprint) |
| Active MITM running an impostor hub | TOFU `hub_pubkey` pinned in `config.toml` |
| Stolen `authorized_keys` by an outsider | No-op — pubkeys are public information |
| Compromised peer (private key extracted) | Bounded — that peer can sync until removed from `authorized_keys` |

### 6.2 Not defended

| Attacker | Reason |
|---|---|
| **Compromised hub (host root)** | Hub holds plaintext content. Out of scope. |
| **Compromised peer until convergence** | Removal from `authorized_keys` is eventually consistent; a peer with a stale copy **MAY** still accept the removed peer until convergence. |
| **Disk theft / backup theft** | Files on disk are plaintext (markdown by design). Use full-disk encryption on every machine running agentsync. |
| **Quantum-capable adversary** | TLS 1.3 + ed25519 are not PQ-safe. A future spec version **MAY** introduce hybrid PQ key exchange via the `agentsync-auth-v1` → `v2` tag bump. |
| **Denial of service** | The hub does not rate-limit handshakes. A reimplementation **SHOULD** apply standard transport-layer rate limits (proxy-level or OS-level). |

### 6.3 Operational guidance

- Treat the hub as a trusted compute node. Run it in a controlled
  environment (your own VM, or a managed service whose operator you
  trust).
- Use full-disk encryption on every device with a vault checkout.
- Rotate identity keys by removing the old line from
  `authorized_keys` (eventually consistent) and adding a new one.
- Monitor `authorized_keys` for unexpected additions; this is the
  audit trail.

---

## 7. Cryptographic primitives

| Primitive | Algorithm |
|---|---|
| Identity signature | Ed25519 (RFC 8032) |
| Hash for cert fingerprint | SHA-256 |
| Hash for blob naming    | SHA-256 |
| Hash for Automerge change IDs | SHA-256 (Automerge internal) |
| RNG for nonces and seeds | OS CSPRNG (`OsRng` in the reference) |
| TLS suite               | TLS 1.3 with rustls defaults |

A reimplementation **MUST NOT** substitute weaker primitives. It
**MAY** add post-quantum hybrid layers under a new transcript domain
tag in a future spec version.

---

## 8. Migration from the symmetric-key model

Earlier drafts of agentsync used a single shared `vault_key` and an
HMAC-derived auth token. That model is **removed**. agentsync is
pre-release; there is no migration tooling. Existing vaults using the
old model **MUST** be re-initialized.

A reimplementation interoperating with current code **MUST NOT** look
for an `AGENTSYNC_KEY` environment variable, a `--vault-key` flag, or
a `[key]` section in `config.toml`. The current model is identity-key
based throughout.

---

## 9. Cross-references

- [WIRE.md § 4](./WIRE.md#handshake-normative) — byte-level handshake.
- [STORAGE.md § 7](./STORAGE.md#authorized_keys-synced) — file format.
- [STORAGE.md § 8](./STORAGE.md#identity-files) — identity file format.
- [STORAGE.md § 5](./STORAGE.md#tls-material-hub-only) — TLS files.
- [DOCUMENT.md § 6](./DOCUMENT.md#authorized_keys) — `authorized_keys`
  representation in the Automerge document.
- [HOST.md § 3.2](./HOST.md#32-signer) — `Signer` trait.
