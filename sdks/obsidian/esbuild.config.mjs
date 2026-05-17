// Build the Obsidian plugin to a single CJS bundle (`main.js`) consumable
// by Obsidian on desktop AND mobile.
//
// The agentsync wasm is read from the SDK's freshly-built `web-pkg/` and
// inlined as a base64 constant under the global `__AGENTSYNC_WASM_B64__`,
// so the plugin can call `initAgentsync()` synchronously without fetching
// at runtime (mobile WebViews can't fetch arbitrary local URLs).

import { existsSync, readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import esbuild from 'esbuild';

const __dirname = dirname(fileURLToPath(import.meta.url));
const prod = process.argv.includes('production');

const wasmPath = resolve(
  __dirname,
  '..',
  'typescript',
  'dist',
  'web-pkg',
  'agentsync_wasm_bg.wasm',
);
if (!existsSync(wasmPath)) {
  console.error(
    `[esbuild] missing wasm at ${wasmPath}\n` +
      `Run \`bun run build:wasm\` (or \`bun run build\`) inside sdks/typescript first.`,
  );
  process.exit(1);
}
const wasmB64 = readFileSync(wasmPath).toString('base64');

const banner = `/*
  Agentsync Obsidian Plugin — bundled by esbuild.
  Source: https://github.com/cjroth/agentsync (sdks/obsidian/)
*/`;

const buildOpts = {
  banner: { js: banner },
  entryPoints: ['src/main.ts'],
  bundle: true,
  format: 'cjs',
  target: 'ES2020',
  platform: 'browser',
  external: [
    'obsidian',
    'electron',
    // Node builtins are reached only on desktop via a guarded require()
    // (NodeHomeIdentityIO); externalize so the browser/mobile bundle never
    // tries to resolve them.
    'node:fs',
    'node:os',
    'node:path',
    '@codemirror/autocomplete',
    '@codemirror/collab',
    '@codemirror/commands',
    '@codemirror/language',
    '@codemirror/lint',
    '@codemirror/search',
    '@codemirror/state',
    '@codemirror/view',
    '@lezer/common',
    '@lezer/highlight',
    '@lezer/lr',
  ],
  define: {
    __AGENTSYNC_WASM_B64__: JSON.stringify(wasmB64),
    'process.env.NODE_ENV': JSON.stringify(prod ? 'production' : 'development'),
    // The wasm-pack `web` glue has a dead-code default-input branch that
    // touches `import.meta.url`; we always pass bytes explicitly, but
    // esbuild still warns about the reference under format=cjs. Replace
    // the read with a literal so the bundler is happy.
    'import.meta.url': JSON.stringify('agentsync-plugin://main'),
  },
  outfile: 'main.js',
  sourcemap: prod ? false : 'inline',
  treeShaking: true,
  minify: prod,
  logLevel: 'info',
};

if (prod) {
  await esbuild.build(buildOpts);
} else {
  const ctx = await esbuild.context(buildOpts);
  await ctx.watch();
  console.log('[esbuild] watching for changes…');
}
