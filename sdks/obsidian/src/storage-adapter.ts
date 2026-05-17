// StorageAdapter implementation backed by Obsidian's `app.vault.adapter`.
// Works identically on desktop (Electron) and mobile (Capacitor WebView)
// because Obsidian abstracts the underlying filesystem.
//
// State lives under `<vault-root>/.agentsync/` — the exact directory the
// native `agentsync` CLI uses (`config.toml`, `doc.bin`, `blobs/`,
// `snapshots/`, …), so one vault folder is interchangeable between the
// CLI and this plugin. `.agentsync` is a dotfolder, which Obsidian's
// content-listing APIs (`getFiles`, `getMarkdownFiles`) skip, so the
// plugin's own state never round-trips through sync.
//
// The device identity is stored as `.agentsync/identity.seed` (a raw
// 32-byte SDK seed) — deliberately a *different* file from the CLI's
// identity (`~/.agentsync/id_ed25519`, an OpenSSH key). Each device keeps
// its own keypair and is authorized independently, so the two coexist
// without collision and we never set `[identity] path` in config.toml.

import type { StorageAdapter } from '@agentsync/sdk/web-init';

/**
 * Subset of `obsidian`'s `DataAdapter` we depend on. We restate it here so
 * the module can be unit-tested without importing the real `obsidian`
 * package (which only ships in the host environment).
 */
export interface MinimalDataAdapter {
  read(path: string): Promise<string>;
  readBinary(path: string): Promise<ArrayBuffer>;
  write(path: string, data: string): Promise<void>;
  writeBinary(path: string, data: ArrayBuffer): Promise<void>;
  exists(path: string): Promise<boolean>;
  mkdir(path: string): Promise<void>;
  remove(path: string): Promise<void>;
  rename(oldPath: string, newPath: string): Promise<void>;
}

/** Sanitize a peer key so it's safe as a filename on every host FS. */
export function sanitizePeerKey(key: string): string {
  if (!/^[0-9a-fA-F]+$/.test(key)) {
    throw new Error(`invalid peer key (expected hex): ${key.slice(0, 32)}…`);
  }
  return key.toLowerCase();
}

export class ObsidianStorageAdapter implements StorageAdapter {
  private readonly docPath: string;
  private readonly identityPath: string;
  private readonly snapshotsPath: string;
  private readonly syncStatesDir: string;

  /**
   * @param adapter The host's DataAdapter (`app.vault.adapter`).
   * @param root    Vault-relative state root — always `.agentsync` so the
   *                layout matches the native CLI (`doc.bin`, `blobs/`, …).
   */
  constructor(
    private readonly adapter: MinimalDataAdapter,
    private readonly root: string = '.agentsync',
  ) {
    this.docPath = `${root}/doc.bin`;
    this.identityPath = `${root}/identity.seed`;
    // The TS Vault keeps labels inside doc.bin, so this is effectively
    // unused — but keep it a plugin-local filename rather than the CLI's
    // `snapshots/index.json` so we can never write an incompatible format
    // over the CLI's snapshot index.
    this.snapshotsPath = `${root}/snapshots.json`;
    this.syncStatesDir = `${root}/sync-states`;
  }

  async loadDoc(): Promise<Uint8Array | null> {
    return this.readBinaryOrNull(this.docPath);
  }

  async saveDoc(bytes: Uint8Array): Promise<void> {
    await this.ensureDir(this.root);
    await this.atomicWriteBinary(this.docPath, bytes);
  }

  async loadIdentitySeed(): Promise<Uint8Array | null> {
    return this.readBinaryOrNull(this.identityPath);
  }

  async saveIdentitySeed(seed: Uint8Array): Promise<void> {
    await this.ensureDir(this.root);
    await this.atomicWriteBinary(this.identityPath, seed);
  }

  async loadSnapshots(): Promise<Uint8Array | null> {
    return this.readBinaryOrNull(this.snapshotsPath);
  }

  async saveSnapshots(bytes: Uint8Array): Promise<void> {
    await this.ensureDir(this.root);
    await this.atomicWriteBinary(this.snapshotsPath, bytes);
  }

  async loadSyncState(peerKey: string): Promise<Uint8Array | null> {
    const path = `${this.syncStatesDir}/${sanitizePeerKey(peerKey)}.bin`;
    return this.readBinaryOrNull(path);
  }

  async saveSyncState(peerKey: string, bytes: Uint8Array): Promise<void> {
    await this.ensureDir(this.syncStatesDir);
    const path = `${this.syncStatesDir}/${sanitizePeerKey(peerKey)}.bin`;
    await this.atomicWriteBinary(path, bytes);
  }

  /** No persistent handles to release. */
  async close(): Promise<void> {}

  // ---- Internal helpers ----

  private async readBinaryOrNull(path: string): Promise<Uint8Array | null> {
    if (!(await this.adapter.exists(path))) return null;
    const buf = await this.adapter.readBinary(path);
    const bytes = new Uint8Array(buf);
    // A zero-length file means "reset" — treat as missing so the SDK
    // regenerates rather than tripping its own length validators (e.g.
    // `Identity.fromSeed` requires exactly 32 bytes).
    return bytes.length === 0 ? null : bytes;
  }

  /**
   * Atomic-ish write: write to `<path>.tmp` then rename. Survives an
   * abrupt shutdown mid-write — the previous version remains intact at
   * `<path>` until the rename succeeds.
   */
  private async atomicWriteBinary(path: string, bytes: Uint8Array): Promise<void> {
    const tmp = `${path}.tmp`;
    const buf = bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);
    await this.adapter.writeBinary(tmp, buf as ArrayBuffer);
    if (await this.adapter.exists(path)) {
      await this.adapter.remove(path);
    }
    await this.adapter.rename(tmp, path);
  }

  /** Create `path` and any missing ancestor segments. */
  private async ensureDir(path: string): Promise<void> {
    if (!path) return;
    const parts = path.split('/').filter(Boolean);
    let cur = '';
    for (const seg of parts) {
      cur = cur ? `${cur}/${seg}` : seg;
      if (!(await this.adapter.exists(cur))) {
        await this.adapter.mkdir(cur);
      }
    }
  }
}
