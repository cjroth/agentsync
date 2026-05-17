// Global test setup — preloaded by `bunfig.toml` before any test files
// are imported. Initializes the agentsync WASM module once so tests can
// call SDK primitives directly without each file repeating the dance.
//
// Plugin sources are arranged so unit tests never transitively import the
// real `obsidian` package (which ships only type declarations):
//   - settings.ts holds the pure schema; settings-tab.ts holds the UI.
//   - main.ts and ui/* import obsidian and are exercised in e2e tests
//     against a real Obsidian-backed test harness.

import { readFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { initAgentsync, isInitialized } from '@agentsync/sdk/web-init';

const setupDir = dirname(fileURLToPath(import.meta.url));
const wasmPath = resolve(
  setupDir,
  '..',
  '..',
  'typescript',
  'dist',
  'web-pkg',
  'agentsync_wasm_bg.wasm',
);

if (!isInitialized()) {
  const bytes = await readFile(wasmPath);
  await initAgentsync(bytes);
}
