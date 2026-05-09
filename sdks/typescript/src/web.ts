// Browser / bundler entry. Uses the wasm-pack `bundler` target glue, which
// emits a top-level `import` of the .wasm file that Vite, webpack, Rollup,
// and esbuild understand. For Node/Bun, import `@agentsync/sdk` instead.

import * as wasm from '#wasm-bundler';
import { type CreateOptions, type OpenOptions, Vault as VaultClass } from './vault.js';
import { wrap } from './wrapper.js';

const wasmModule = wrap(wasm);

export const {
  Identity,
  Pubkey,
  Doc,
  SyncState,
  parseAuthorizedKeys,
  renderAuthorizedKeys,
  randomNonce,
  buildTranscript,
  encodeFrame,
  decodeFrame,
  contentHash,
  schemaVersion,
  defaultPort,
  normalizeRendezvousUrl,
} = wasmModule;

/** High-level Vault API. The default transport uses the global
 * `WebSocket` constructor (every modern browser provides one). Pass
 * `transport` explicitly for non-standard runtimes. */
export const Vault = {
  create(opts: CreateOptions) {
    return VaultClass.create(wasmModule, opts);
  },
  open(opts: OpenOptions) {
    return VaultClass.open(wasmModule, opts);
  },
};

export type { Vault as VaultInstance, CreateOptions, OpenOptions } from './vault.js';
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

export { MemoryStorage, memoryStorage } from './adapters/memory-storage.js';
export { OpfsStorage, opfsStorage } from './adapters/opfs-storage.js';
