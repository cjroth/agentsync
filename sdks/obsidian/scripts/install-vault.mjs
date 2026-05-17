// Copy the built plugin into an Obsidian vault's plugins folder.
//
//   OBSIDIAN_VAULT=/path/to/vault bun run install:vault
//
// Run after `bun run build` (the `install:vault` script chains both).
// Copies main.js + manifest.json (+ styles.css if present) into
// <vault>/.obsidian/plugins/<manifest.id>/ and leaves the plugin's
// `.agentsync/` state untouched.

import { copyFileSync, existsSync, mkdirSync, readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const pluginRoot = resolve(here, '..');

const vault = process.env.OBSIDIAN_VAULT;
if (!vault) {
  console.error(
    'install:vault: set OBSIDIAN_VAULT to your Obsidian vault root.\n' +
      '  e.g. OBSIDIAN_VAULT="/Users/you/Notes" bun run install:vault',
  );
  process.exit(1);
}
if (!existsSync(join(vault, '.obsidian'))) {
  console.error(
    `install:vault: ${vault} doesn't look like an Obsidian vault ` +
      "(no .obsidian/ directory). Check the OBSIDIAN_VAULT path.",
  );
  process.exit(1);
}

const { id } = JSON.parse(readFileSync(join(pluginRoot, 'manifest.json'), 'utf8'));
const dest = join(vault, '.obsidian', 'plugins', id);
mkdirSync(dest, { recursive: true });

for (const f of ['main.js', 'manifest.json', 'styles.css']) {
  const src = join(pluginRoot, f);
  if (!existsSync(src)) {
    if (f === 'styles.css') continue; // optional
    console.error(`install:vault: ${f} missing — run \`bun run build\` first.`);
    process.exit(1);
  }
  copyFileSync(src, join(dest, f));
  console.log(`  ${f} → ${join(dest, f)}`);
}
console.log(
  `install:vault: installed "${id}". Reload Obsidian (or toggle the ` +
    'plugin off/on) to pick it up.',
);
