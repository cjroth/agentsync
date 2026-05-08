// Browser / bundler entry. Uses the wasm-pack `bundler` target glue, which
// emits a top-level `import` of the .wasm file that Vite, webpack, Rollup,
// and esbuild understand. For Node/Bun, import `@agentsync/sdk` instead.

import * as wasm from '#wasm-bundler';
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
