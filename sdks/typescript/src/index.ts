// Default entry. Targets Node and Bun via the wasm-pack `nodejs` glue.
// Browser / bundler consumers should import from `@agentsync/sdk/web`,
// which uses the wasm-pack `bundler` glue and lets Vite, webpack, Rollup,
// and esbuild handle the .wasm import.
//
// Both entry points expose the same TypeScript surface.

import * as wasm from '#wasm-nodejs';
import {
  type CreateOptions,
  type OpenOptions,
  type ProbeOptions,
  Vault as VaultClass,
} from './vault.js';
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

/** High-level Vault API (sync, watch, restore, labels). */
export const Vault = {
  create(opts: CreateOptions) {
    return VaultClass.create(wasmModule, opts);
  },
  open(opts: OpenOptions) {
    return VaultClass.open(wasmModule, opts);
  },
  /** Discover a hub's vault id (and identity) without joining. */
  probeHub(opts: ProbeOptions) {
    return VaultClass.probeHub(wasmModule, opts);
  },
};

export type { Vault as VaultInstance } from './vault.js';
export type { CreateOptions, HubInfo, OpenOptions, ProbeOptions } from './vault.js';
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
export { NodeFsStorage, nodeFsStorage } from './adapters/node-fs-storage.js';
export { nodeWsTransport } from './adapters/ws-transport-node.js';

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
