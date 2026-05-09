// In-memory StorageAdapter — useful for tests and ephemeral browser
// scenarios. Persistence vanishes when the instance is dropped.

import type { StorageAdapter } from '../types.js';

export class MemoryStorage implements StorageAdapter {
  private doc: Uint8Array | null = null;
  private syncStates = new Map<string, Uint8Array>();
  private identitySeed: Uint8Array | null = null;
  private snapshots: Uint8Array | null = null;

  async loadDoc(): Promise<Uint8Array | null> {
    return this.doc ? new Uint8Array(this.doc) : null;
  }
  async saveDoc(bytes: Uint8Array): Promise<void> {
    this.doc = new Uint8Array(bytes);
  }
  async loadSyncState(peerKey: string): Promise<Uint8Array | null> {
    const v = this.syncStates.get(peerKey);
    return v ? new Uint8Array(v) : null;
  }
  async saveSyncState(peerKey: string, bytes: Uint8Array): Promise<void> {
    this.syncStates.set(peerKey, new Uint8Array(bytes));
  }
  async loadIdentitySeed(): Promise<Uint8Array | null> {
    return this.identitySeed ? new Uint8Array(this.identitySeed) : null;
  }
  async saveIdentitySeed(seed: Uint8Array): Promise<void> {
    this.identitySeed = new Uint8Array(seed);
  }
  async loadSnapshots(): Promise<Uint8Array | null> {
    return this.snapshots ? new Uint8Array(this.snapshots) : null;
  }
  async saveSnapshots(bytes: Uint8Array): Promise<void> {
    this.snapshots = new Uint8Array(bytes);
  }
  async close(): Promise<void> {}
}

/** Convenience constructor — `memoryStorage()` is more readable than `new MemoryStorage()`. */
export function memoryStorage(): MemoryStorage {
  return new MemoryStorage();
}
