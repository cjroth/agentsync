// Node-side StorageAdapter that mirrors the on-disk layout of the Rust
// CLI: <root>/doc.bin, <root>/sync-states/<peerKey>.bin,
// <root>/identity.seed, <root>/snapshots.json. Atomic writes use the
// write-tmp-then-rename pattern.

import { mkdir, readFile, rename, rm, writeFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import type { StorageAdapter } from '../types.js';

async function readBytes(path: string): Promise<Uint8Array | null> {
  try {
    const buf = await readFile(path);
    return new Uint8Array(buf.buffer, buf.byteOffset, buf.byteLength);
  } catch (e: unknown) {
    if ((e as NodeJS.ErrnoException).code === 'ENOENT') return null;
    throw e;
  }
}

async function writeAtomic(path: string, bytes: Uint8Array): Promise<void> {
  await mkdir(dirname(path), { recursive: true });
  const tmp = `${path}.tmp-${process.pid}-${Date.now()}`;
  await writeFile(tmp, bytes);
  try {
    await rename(tmp, path);
  } catch (e) {
    await rm(tmp, { force: true }).catch(() => {});
    throw e;
  }
}

export class NodeFsStorage implements StorageAdapter {
  constructor(private root: string) {}

  async loadDoc(): Promise<Uint8Array | null> {
    return readBytes(join(this.root, 'doc.bin'));
  }
  async saveDoc(bytes: Uint8Array): Promise<void> {
    await writeAtomic(join(this.root, 'doc.bin'), bytes);
  }
  async loadSyncState(peerKey: string): Promise<Uint8Array | null> {
    return readBytes(join(this.root, 'sync-states', `${peerKey}.bin`));
  }
  async saveSyncState(peerKey: string, bytes: Uint8Array): Promise<void> {
    await writeAtomic(join(this.root, 'sync-states', `${peerKey}.bin`), bytes);
  }
  async loadIdentitySeed(): Promise<Uint8Array | null> {
    return readBytes(join(this.root, 'identity.seed'));
  }
  async saveIdentitySeed(seed: Uint8Array): Promise<void> {
    await writeAtomic(join(this.root, 'identity.seed'), seed);
  }
  async loadSnapshots(): Promise<Uint8Array | null> {
    return readBytes(join(this.root, 'snapshots.json'));
  }
  async saveSnapshots(bytes: Uint8Array): Promise<void> {
    await writeAtomic(join(this.root, 'snapshots.json'), bytes);
  }
  async close(): Promise<void> {}
}

export function nodeFsStorage(root: string): NodeFsStorage {
  return new NodeFsStorage(root);
}
