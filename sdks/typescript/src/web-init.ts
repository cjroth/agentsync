// Explicit-init browser entry. Use this when your bundler can't follow the
// top-level `import './x_bg.wasm'` that the wasm-pack `bundler` target glue
// emits — most commonly a CJS-target esbuild build (Obsidian plugins,
// VS Code extensions, etc.). The host inlines the .wasm bytes itself and
// passes them to `initAgentsync()` once at startup; everything else then
// works exactly like `@agentsync/sdk/web`.
//
// For Vite / webpack / Rollup / native ESM environments prefer
// `@agentsync/sdk/web` — the bundler picks up the .wasm automatically.
//
//     import { initAgentsync, Vault, memoryStorage } from '@agentsync/sdk/web-init';
//     // wasmBytes is a Uint8Array / ArrayBuffer / Response / URL
//     await initAgentsync(wasmBytes);
//     const v = await Vault.create({ storage: memoryStorage(), rendezvousUrl: 'wss://hub' });

import init, * as wasm from '#wasm-web';
import {
  type CreateOptions,
  type OpenOptions,
  type ProbeOptions,
  Vault as VaultClass,
} from './vault.js';
import { wrap } from './wrapper.js';

type WasmInput = Parameters<typeof init>[0];
type WasmModule = ReturnType<typeof wrap>;

let mod: WasmModule | null = null;

function assertReady(): WasmModule {
  if (!mod) {
    throw new Error(
      'agentsync: WASM not initialized — call `await initAgentsync(wasmBytes)` first',
    );
  }
  return mod;
}

/**
 * Load and initialize the WASM module. Idempotent — additional calls after
 * the first are no-ops and resolve immediately.
 *
 * `input` accepts anything wasm-bindgen's `web` target accepts: a
 * `BufferSource` (Uint8Array / ArrayBuffer), a `Response`, a `URL`, a
 * `WebAssembly.Module`, or a string URL.
 */
export async function initAgentsync(input: WasmInput): Promise<void> {
  if (mod) return;
  // wasm-bindgen ≥ 0.2.93 prefers the `{ module_or_path }` object form. We
  // accept the legacy bare value here for ergonomic callers and re-wrap.
  // biome-ignore lint/suspicious/noExplicitAny: input shape is provider-defined
  await init({ module_or_path: input as any });
  mod = wrap(wasm);
}

/** True once the WASM is loaded and ready to use. */
export function isInitialized(): boolean {
  return mod !== null;
}

/** High-level Vault factory — same API as `@agentsync/sdk/web`. */
export const Vault = {
  create(opts: CreateOptions) {
    return VaultClass.create(assertReady(), opts);
  },
  open(opts: OpenOptions) {
    return VaultClass.open(assertReady(), opts);
  },
  /** Discover a hub's vault id (and identity) without joining. */
  probeHub(opts: ProbeOptions) {
    return VaultClass.probeHub(assertReady(), opts);
  },
};

// ---- Lazy primitive accessors ----
//
// Each accessor calls `assertReady()` so calling them before init throws a
// descriptive error rather than blowing up deep inside wasm-bindgen with a
// confusing "wasm is undefined" message.

export const Identity = {
  generate() {
    return assertReady().Identity.generate();
  },
  fromSeed(seed: Uint8Array) {
    return assertReady().Identity.fromSeed(seed);
  },
};

export const Pubkey = {
  fromBytes(bytes: Uint8Array) {
    return assertReady().Pubkey.fromBytes(bytes);
  },
  fromSshString(s: string) {
    return assertReady().Pubkey.fromSshString(s);
  },
};

export const SyncState = {
  create() {
    return new (assertReady().SyncState)();
  },
  decode(bytes: Uint8Array) {
    return assertReady().SyncState.decode(bytes);
  },
};

export const Doc = {
  create(vaultId: string) {
    return new (assertReady().Doc)(vaultId);
  },
  load(bytes: Uint8Array) {
    return assertReady().Doc.load(bytes);
  },
};

export const parseAuthorizedKeys = (body: string) => assertReady().parseAuthorizedKeys(body);
export const renderAuthorizedKeys: WasmModule['renderAuthorizedKeys'] = (entries) =>
  assertReady().renderAuthorizedKeys(entries);
export const randomNonce = () => assertReady().randomNonce();
export const buildTranscript: WasmModule['buildTranscript'] = (
  hubNonce,
  peerNonce,
  tlsCertFingerprint,
  hubPubkey,
  peerPubkey,
) => assertReady().buildTranscript(hubNonce, peerNonce, tlsCertFingerprint, hubPubkey, peerPubkey);
export const encodeFrame: WasmModule['encodeFrame'] = (frame) => assertReady().encodeFrame(frame);
export const decodeFrame: WasmModule['decodeFrame'] = (bytes) => assertReady().decodeFrame(bytes);
export const contentHash = (bytes: Uint8Array) => assertReady().contentHash(bytes);
export const schemaVersion = () => assertReady().schemaVersion();
export const defaultPort = () => assertReady().defaultPort();
export const normalizeRendezvousUrl = (url: string) => assertReady().normalizeRendezvousUrl(url);

// ---- Re-exports ----

export type {
  CreateOptions,
  HubInfo,
  OpenOptions,
  ProbeOptions,
  Vault as VaultInstance,
} from './vault.js';
export type {
  AuthorizedPeer,
  DirectoryMeta,
  FileMeta,
  Frame,
  FrameTag,
  HelloOp,
  Label,
  ReconnectOptions,
  StorageAdapter,
  TransportAdapter,
  TransportConn,
  VaultEvent,
  VaultOptions,
} from './types.js';

export type {
  Doc as DocInstance,
  Identity as IdentityInstance,
  Pubkey as PubkeyInstance,
  SyncState as SyncStateInstance,
} from './wrapper.js';

export { MemoryStorage, memoryStorage } from './adapters/memory-storage.js';

export {
  type AgentsyncConfig,
  type IdentitySection,
  type SyncSection,
  type TomlDoc,
  type TomlValue,
  type VaultSection,
  applyConfigToDoc,
  configFromDoc,
  defaultConfig,
  defaultSyncSection,
  parseConfig,
  parseTomlDoc,
  serializeConfig,
  stringifyTomlDoc,
} from './config.js';

export {
  formatAgentsyncIdentity,
  formatPubkeySidecar,
  parseAgentsyncIdentity,
} from './identity-file.js';

/** @internal — exposed only for tests. Resets module-level state. */
export function _resetForTests(): void {
  mod = null;
}
