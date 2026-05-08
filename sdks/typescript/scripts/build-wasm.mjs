// Build the wasm crate twice — once for bundlers (web/Vite/webpack/Rollup),
// once for Node — and emit a single ESM `dist/wasm/` directory that holds
// the raw .wasm + the .d.ts as a sibling to the per-target glue. The TS
// wrappers in src/ pick the right glue at import time via subpath exports.
//
// If `wasm-opt` is on PATH it's run on each emitted .wasm with `-Oz`. CI
// installs binaryen; locally it's optional.

import { execFileSync, execSync } from 'node:child_process';
import { copyFileSync, existsSync, mkdirSync, readdirSync, rmSync, statSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const sdkRoot = resolve(__dirname, '..');
const repoRoot = resolve(sdkRoot, '..', '..');
const crate = resolve(repoRoot, 'crates', 'agentsync-wasm');
const distRoot = resolve(sdkRoot, 'dist');

function run(cmd, args, opts = {}) {
  console.log(`$ ${cmd} ${args.join(' ')}`);
  execFileSync(cmd, args, { stdio: 'inherit', ...opts });
}

function maybeWasmOpt(wasmPath) {
  try {
    execSync('wasm-opt --version', { stdio: 'ignore' });
  } catch {
    console.log('  (wasm-opt not on PATH, skipping size optimization)');
    return;
  }
  const tmp = `${wasmPath}.opt`;
  run('wasm-opt', ['-Oz', '--enable-mutable-globals', '-o', tmp, wasmPath]);
  copyFileSync(tmp, wasmPath);
  rmSync(tmp);
}

function buildTarget(target, outName) {
  const out = join(distRoot, outName);
  // wasm-pack rejects non-empty out dirs; clear first.
  rmSync(out, { recursive: true, force: true });
  mkdirSync(out, { recursive: true });
  run('wasm-pack', [
    'build',
    crate,
    '--target',
    target,
    '--release',
    '--out-dir',
    out,
    '--out-name',
    'agentsync_wasm',
  ]);
  for (const entry of readdirSync(out)) {
    if (entry.endsWith('.wasm')) {
      maybeWasmOpt(join(out, entry));
      const sz = statSync(join(out, entry)).size;
      console.log(`  ${entry}: ${(sz / 1024).toFixed(1)} KiB`);
    }
  }
  // wasm-pack writes a package.json at the root we don't want shipping —
  // the consumer only sees @agentsync/sdk's package.json. Leave the file
  // in place (harmless) but the SDK's "files" glob already excludes it.
}

mkdirSync(distRoot, { recursive: true });

buildTarget('bundler', 'bundler');
buildTarget('nodejs', 'nodejs');

// Mirror the raw .wasm into dist/wasm/ so the `./wasm` subpath export
// resolves to a single canonical binary regardless of glue target.
const wasmDir = join(distRoot, 'wasm');
mkdirSync(wasmDir, { recursive: true });
copyFileSync(
  join(distRoot, 'bundler', 'agentsync_wasm_bg.wasm'),
  join(wasmDir, 'agentsync_wasm_bg.wasm'),
);

// Sanity check that the bundler glue exists where the TS wrappers import
// from.
for (const f of [
  'bundler/agentsync_wasm.js',
  'bundler/agentsync_wasm.d.ts',
  'bundler/agentsync_wasm_bg.wasm',
  'nodejs/agentsync_wasm.js',
  'nodejs/agentsync_wasm.d.ts',
  'nodejs/agentsync_wasm_bg.wasm',
]) {
  if (!existsSync(join(distRoot, f))) {
    throw new Error(`expected wasm-pack to emit ${f}`);
  }
}

console.log('\\nwasm build OK');
