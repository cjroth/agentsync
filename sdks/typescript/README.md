# @agentsync/sdk

TypeScript / WebAssembly SDK for [agentsync](https://github.com/cjroth/agentsync).
Wraps the same Rust core that powers the `agentsync` CLI, compiled to wasm32 and shipped with idiomatic TS bindings.

## Install

```bash
npm install @agentsync/sdk
# or
bun add @agentsync/sdk
```

## Usage

```ts
import { Identity, Doc, parseAuthorizedKeys } from '@agentsync/sdk';

const me = Identity.generate();
console.log(me.pubkey().toSshString());
// → ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAA...

const doc = new Doc('vault-1');
doc.writeTextFile('notes/hello.md', '# hello\n');
const bytes = doc.save();           // serialize over the wire / to disk
const reloaded = Doc.load(bytes);
console.log(reloaded.readFile('notes/hello.md'));

const peers = parseAuthorizedKeys(`
  ssh-ed25519 AAAA... alice
  ssh-ed25519 AAAA... bob
`);
```

## Entry points

| Import | Target | Use when |
| --- | --- | --- |
| `@agentsync/sdk` | Node + Bun | server, CLI, tests |
| `@agentsync/sdk/web` | browser bundlers (Vite, webpack, Rollup, esbuild) | frontends |
| `@agentsync/sdk/wasm` | raw `.wasm` bytes | custom loaders, Cloudflare Workers |
| `@agentsync/sdk/wasm/bundler` | bundler glue + types | when you want the wasm-bindgen surface directly |

All entry points expose the same TypeScript API.

## API surface

The SDK mirrors the wasm-safe slice of the Rust `agentsync-core` crate:

- **`Identity`** — file-backed ed25519 keypair (`generate`, `fromSeed`, `seed`, `sign`, `pubkey`)
- **`Pubkey`** — SSH-style serialization (`toSshString`, `fromSshString`, `fingerprint`, `verify`)
- **`Doc`** — Automerge-backed vault document (`writeTextFile`, `readFile`, `listFiles`, `merge`, `save`, `load`)
- **`parseAuthorizedKeys` / `renderAuthorizedKeys`** — SSH-style auth file
- **`encodeFrame` / `decodeFrame`** — msgpack codec for the wire protocol
- **`buildTranscript` / `randomNonce`** — handshake helpers
- **`contentHash`, `schemaVersion`, `defaultPort`, `normalizeRendezvousUrl`** — core helpers

Networking and on-disk storage are intentionally NOT in this SDK — those
live in the native CLI. To build a peer in JS land, open a `WebSocket` (or
`ws` on Node) to a hub and feed bytes through `encodeFrame` / `decodeFrame`
+ `buildTranscript` + `Identity.sign`.

## Memory management

The exported classes are wasm-bindgen wrappers around pointers in linear
memory. They support the `using` declaration if your runtime has the
explicit-resource-management proposal; otherwise call `.free()` when
done:

```ts
{
  using id = Identity.generate();
  // ...
}            // automatically freed

const doc = new Doc('v');
try {
  // ...
} finally {
  doc.free();
}
```

## Develop

```bash
bun install
bun run build       # wasm-pack (bundler + nodejs targets) + tsc
bun test            # unit tests
bun run lint        # biome
bun run typecheck
AGENTSYNC_BIN=path/to/agentsync bun run test:e2e
```

The e2e suite runs under Node (not Bun) because Bun's WebSocket client
doesn't currently support the hub's ed25519 self-signed TLS cert. Unit
tests run under Bun.

## Supply chain

`bunfig.toml` sets `minimumReleaseAge = 604800` so `bun install` refuses
any npm package whose latest version is less than 7 days old. This
blocks the typical short-lived poisoning window from a stolen
maintainer token before it reaches the lockfile. To bypass for a
specific incident, add the package name to `minimumReleaseAgeExcludes`
in `bunfig.toml` — don't disable globally.

## License

MIT or Apache-2.0, at your option.
