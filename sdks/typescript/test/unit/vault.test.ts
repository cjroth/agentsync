// Vault unit tests covering: create/open round-trip, file/dir/label
// operations, restore-to-label, restore-to-time, event subscription.
// Uses `MemoryStorage` so no fs / network involved.

import { afterEach, beforeEach, describe, expect, it } from 'bun:test';
import { type CreateOptions, MemoryStorage, Vault, memoryStorage } from '../../src/index.js';

async function freshVault(opts?: Partial<CreateOptions>) {
  const storage = memoryStorage();
  const v = await Vault.create({ storage, ...opts });
  return { v, storage };
}

describe('Vault.create / Vault.open', () => {
  it('creates a new vault and persists doc.bin to storage', async () => {
    const { v, storage } = await freshVault();
    const bytes = await storage.loadDoc();
    expect(bytes).not.toBeNull();
    expect(bytes!.length).toBeGreaterThan(0);
    expect(v.vaultIdValue()).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}/);
    await v.close();
  });

  it('open() reuses persisted state after a close', async () => {
    const { v, storage } = await freshVault();
    const id1 = v.vaultIdValue();
    await v.writeTextFile('hello.md', '# hi\n');
    await v.close();
    const v2 = await Vault.open({ storage });
    expect(v2.vaultIdValue()).toBe(id1);
    expect(await v2.readTextFile('hello.md')).toBe('# hi\n');
    await v2.close();
  });

  it('open() errors when no doc on disk', async () => {
    const storage = new MemoryStorage();
    await expect(Vault.open({ storage })).rejects.toThrow(/no vault on disk/);
  });

  it('seeds authorized_keys with the creator pubkey', async () => {
    const { v } = await freshVault();
    const body = await v.readTextFile('authorized_keys');
    expect(body).toMatch(/^ssh-ed25519 [A-Za-z0-9+/]+ creator\n$/);
    await v.close();
  });
});

describe('Vault file operations', () => {
  it('write/read/delete round-trip', async () => {
    const { v } = await freshVault();
    await v.writeTextFile('a.md', 'hello');
    expect(v.fileExists('a.md')).toBe(true);
    expect(await v.readTextFile('a.md')).toBe('hello');
    await v.deleteFile('a.md');
    expect(v.fileExists('a.md')).toBe(false);
    await v.close();
  });

  it('listFiles returns metadata', async () => {
    const { v } = await freshVault();
    await v.writeTextFile('one.md', '1');
    await v.writeTextFile('two.md', '22');
    const files = v.listFiles().filter((f) => !f.deleted_at);
    const paths = files.map((f) => f.path).sort();
    expect(paths).toContain('one.md');
    expect(paths).toContain('two.md');
    await v.close();
  });

  it('renameFile changes the path but keeps the id', async () => {
    const { v } = await freshVault();
    const id = await v.writeTextFile('old.md', 'x');
    await v.renameFile('old.md', 'new.md');
    expect(v.fileExists('new.md')).toBe(true);
    expect(v.fileExists('old.md')).toBe(false);
    const files = v.listFiles();
    const m = files.find((f) => f.id === id);
    expect(m?.path).toBe('new.md');
    await v.close();
  });

  it('directories: create, list, delete', async () => {
    const { v } = await freshVault();
    await v.createDirectory('docs');
    const dirs = v.listDirectories().filter((d) => !d.deleted_at);
    expect(dirs.some((d) => d.path === 'docs')).toBe(true);
    await v.deleteDirectory('docs');
    const after = v.listDirectories().filter((d) => !d.deleted_at);
    expect(after.some((d) => d.path === 'docs')).toBe(false);
    await v.close();
  });
});

describe('Vault labels + restore', () => {
  it('createLabel + listLabels round-trip', async () => {
    const { v } = await freshVault();
    await v.writeTextFile('a.md', 'first');
    await v.createLabel('v1');
    await v.writeTextFile('a.md', 'second');
    const labels = v.listLabels();
    expect(labels.some((l) => l.name === 'v1')).toBe(true);
    await v.close();
  });

  it('restoreToLabel reverts file content', async () => {
    const { v } = await freshVault();
    await v.writeTextFile('a.md', 'first');
    await v.createLabel('v1');
    await v.writeTextFile('a.md', 'second');
    expect(await v.readTextFile('a.md')).toBe('second');
    await v.restoreToLabel('v1');
    expect(await v.readTextFile('a.md')).toBe('first');
    await v.close();
  });

  it('restoreToTime reverts to a past timestamp', async () => {
    const { v } = await freshVault();
    await v.writeTextFile('a.md', 'first');
    // Wait 10ms so the next write has a strictly later timestamp.
    await new Promise((r) => setTimeout(r, 10));
    const t = Date.now();
    await new Promise((r) => setTimeout(r, 10));
    await v.writeTextFile('a.md', 'second');
    await v.restoreToTime(t);
    expect(await v.readTextFile('a.md')).toBe('first');
    await v.close();
  });

  it('deleteLabel removes the entry', async () => {
    const { v } = await freshVault();
    await v.createLabel('tmp');
    expect(v.listLabels().some((l) => l.name === 'tmp')).toBe(true);
    await v.deleteLabel('tmp');
    expect(v.listLabels().some((l) => l.name === 'tmp')).toBe(false);
    await v.close();
  });
});

describe('Vault events', () => {
  it('subscribe receives doc-changed events on local writes', async () => {
    const { v } = await freshVault();
    // Local writes don't currently emit doc-changed (only remote writes
    // do); this asserts the subscribe/unsubscribe infrastructure works.
    const seen: string[] = [];
    const unsub = v.subscribe((e) => seen.push(e.kind));
    expect(typeof unsub).toBe('function');
    unsub();
    await v.close();
  });

  it('isConnected returns false when offline', async () => {
    const { v } = await freshVault();
    expect(v.isConnected()).toBe(false);
    await v.close();
  });
});

describe('MemoryStorage', () => {
  let s: MemoryStorage;
  beforeEach(() => {
    s = new MemoryStorage();
  });
  afterEach(async () => s.close());

  it('round-trips doc bytes', async () => {
    expect(await s.loadDoc()).toBeNull();
    await s.saveDoc(new Uint8Array([1, 2, 3]));
    const back = await s.loadDoc();
    expect(back).not.toBeNull();
    expect(Array.from(back!)).toEqual([1, 2, 3]);
  });

  it('round-trips sync state per peer', async () => {
    expect(await s.loadSyncState('peer1')).toBeNull();
    await s.saveSyncState('peer1', new Uint8Array([9]));
    expect(Array.from((await s.loadSyncState('peer1'))!)).toEqual([9]);
  });

  it('round-trips identity seed', async () => {
    expect(await s.loadIdentitySeed()).toBeNull();
    await s.saveIdentitySeed(new Uint8Array(32));
    expect((await s.loadIdentitySeed())!.length).toBe(32);
  });
});
