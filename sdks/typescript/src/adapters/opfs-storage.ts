// Browser-side StorageAdapter using the Origin Private File System (OPFS).
// File-shaped API, multi-GB quota, no permission prompt. Designed to run
// from a Web Worker for sync FileSystemSyncAccessHandle access; falls back
// to async writable streams on the main thread.
//
// Layout under <root>/:
//   doc.bin
//   sync-states/<peerKey>.bin
//   identity.seed
//   snapshots.json

import type { StorageAdapter } from '../types.js';

interface Navigator {
  storage?: { getDirectory(): Promise<FileSystemDirectoryHandle> };
}

declare const navigator: Navigator;

export class OpfsStorage implements StorageAdapter {
  private rootName: string;
  private rootHandle: FileSystemDirectoryHandle | null = null;

  constructor(rootName = 'agentsync') {
    this.rootName = rootName;
  }

  private async ensureRoot(): Promise<FileSystemDirectoryHandle> {
    if (this.rootHandle) return this.rootHandle;
    if (!navigator.storage?.getDirectory) {
      throw new Error('OPFS unavailable in this runtime');
    }
    const root = await navigator.storage.getDirectory();
    this.rootHandle = await root.getDirectoryHandle(this.rootName, { create: true });
    return this.rootHandle;
  }

  private async childDir(...parts: string[]): Promise<FileSystemDirectoryHandle> {
    let handle = await this.ensureRoot();
    for (const part of parts) {
      handle = await handle.getDirectoryHandle(part, { create: true });
    }
    return handle;
  }

  private async readFileBytes(
    parent: FileSystemDirectoryHandle,
    name: string,
  ): Promise<Uint8Array | null> {
    try {
      const fileHandle = await parent.getFileHandle(name);
      const file = await fileHandle.getFile();
      return new Uint8Array(await file.arrayBuffer());
    } catch (e: unknown) {
      if ((e as DOMException).name === 'NotFoundError') return null;
      throw e;
    }
  }

  private async writeFileBytes(
    parent: FileSystemDirectoryHandle,
    name: string,
    bytes: Uint8Array,
  ): Promise<void> {
    const fileHandle = await parent.getFileHandle(name, { create: true });
    // Prefer the sync access handle (Worker-only). Falls back to the async
    // writable stream on the main thread.
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const sync = (fileHandle as any).createSyncAccessHandle as
      | (() => Promise<{
          truncate(size: number): void;
          write(buf: Uint8Array, opts: { at: number }): number;
          flush(): void;
          close(): void;
        }>)
      | undefined;
    if (typeof sync === 'function') {
      const handle = await sync.call(fileHandle);
      try {
        handle.truncate(0);
        handle.write(bytes, { at: 0 });
        handle.flush();
      } finally {
        handle.close();
      }
      return;
    }
    const writable = await fileHandle.createWritable();
    try {
      // Copy into a fresh ArrayBuffer-backed Uint8Array to satisfy the
      // FileSystemWritableFileStream type (which doesn't accept SharedArrayBuffer-backed views).
      const copy = new Uint8Array(bytes.byteLength);
      copy.set(bytes);
      await writable.write(copy);
    } finally {
      await writable.close();
    }
  }

  async loadDoc(): Promise<Uint8Array | null> {
    const root = await this.ensureRoot();
    return this.readFileBytes(root, 'doc.bin');
  }
  async saveDoc(bytes: Uint8Array): Promise<void> {
    const root = await this.ensureRoot();
    await this.writeFileBytes(root, 'doc.bin', bytes);
  }
  async loadSyncState(peerKey: string): Promise<Uint8Array | null> {
    const dir = await this.childDir('sync-states');
    return this.readFileBytes(dir, `${peerKey}.bin`);
  }
  async saveSyncState(peerKey: string, bytes: Uint8Array): Promise<void> {
    const dir = await this.childDir('sync-states');
    await this.writeFileBytes(dir, `${peerKey}.bin`, bytes);
  }
  async loadIdentitySeed(): Promise<Uint8Array | null> {
    const root = await this.ensureRoot();
    return this.readFileBytes(root, 'identity.seed');
  }
  async saveIdentitySeed(seed: Uint8Array): Promise<void> {
    const root = await this.ensureRoot();
    await this.writeFileBytes(root, 'identity.seed', seed);
  }
  async loadSnapshots(): Promise<Uint8Array | null> {
    const root = await this.ensureRoot();
    return this.readFileBytes(root, 'snapshots.json');
  }
  async saveSnapshots(bytes: Uint8Array): Promise<void> {
    const root = await this.ensureRoot();
    await this.writeFileBytes(root, 'snapshots.json', bytes);
  }
  async close(): Promise<void> {
    this.rootHandle = null;
  }
}

export function opfsStorage(rootName?: string): OpfsStorage {
  return new OpfsStorage(rootName);
}
