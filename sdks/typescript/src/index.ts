// Default entry. Targets Node and Bun via the wasm-pack `nodejs` glue.
// Browser / bundler consumers should import from `@agentsync/sdk/web`,
// which uses the wasm-pack `bundler` glue and lets Vite, webpack, Rollup,
// and esbuild handle the .wasm import.
//
// Both entry points expose the exact same TypeScript surface.

import * as wasm from '#wasm-nodejs';
import { wrap } from './wrapper.js';

export const {
  Identity,
  Pubkey,
  Doc,
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
} = wrap(wasm);

export type { AuthorizedPeer, FileMeta, Frame, FrameTag, HelloOp } from './types.js';
