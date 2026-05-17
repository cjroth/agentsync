import { afterEach, beforeEach, describe, expect, test } from 'bun:test';
import { ObsidianStorageAdapter, sanitizePeerKey } from '../../src/storage-adapter.js';
import { FakeDataAdapter } from '../mocks/obsidian.js';

// Matches the CLI: state lives in `<vault-root>/.agentsync/`.
const ROOT = '.agentsync';

let adapter: FakeDataAdapter;
let storage: ObsidianStorageAdapter;

beforeEach(() => {
  adapter = new FakeDataAdapter();
  storage = new ObsidianStorageAdapter(adapter, ROOT);
});

test('defaults its root to .agentsync', async () => {
  const fs = new FakeDataAdapter();
  await new ObsidianStorageAdapter(fs).saveDoc(new Uint8Array([7]));
  expect(await fs.exists('.agentsync/doc.bin')).toBe(true);
});

afterEach(async () => {
  await storage.close();
});

describe('sanitizePeerKey', () => {
  test('lowercases hex', () => {
    expect(sanitizePeerKey('AABB')).toBe('aabb');
  });
  test('rejects non-hex', () => {
    expect(() => sanitizePeerKey('zzzz')).toThrow(/invalid peer key/);
    expect(() => sanitizePeerKey('../etc/passwd')).toThrow();
  });
});

describe('ObsidianStorageAdapter — doc.bin', () => {
  test('loadDoc returns null when missing', async () => {
    expect(await storage.loadDoc()).toBeNull();
  });
  test('saveDoc writes atomically and roundtrips through loadDoc', async () => {
    await storage.saveDoc(new Uint8Array([1, 2, 3]));
    const back = await storage.loadDoc();
    expect(back).not.toBeNull();
    expect(Array.from(back!)).toEqual([1, 2, 3]);
  });
  test('saveDoc twice replaces previous bytes (atomic-rename branch)', async () => {
    await storage.saveDoc(new Uint8Array([1]));
    await storage.saveDoc(new Uint8Array([2, 2]));
    const back = await storage.loadDoc();
    expect(Array.from(back!)).toEqual([2, 2]);
  });
  test('saveDoc creates the .agentsync root', async () => {
    await storage.saveDoc(new Uint8Array([0]));
    expect(await adapter.exists(ROOT)).toBe(true);
    expect(await adapter.exists(`${ROOT}/doc.bin`)).toBe(true);
  });
  test('mkdir is recursive (creates nested parents)', async () => {
    // sync-states sits one level under the root.
    await storage.saveSyncState('aabb', new Uint8Array([1]));
    expect(await adapter.exists(ROOT)).toBe(true);
    expect(await adapter.exists(`${ROOT}/sync-states`)).toBe(true);
  });
});

describe('ObsidianStorageAdapter — identity / snapshots', () => {
  test('loadIdentitySeed null then roundtrips', async () => {
    expect(await storage.loadIdentitySeed()).toBeNull();
    await storage.saveIdentitySeed(new Uint8Array(32).fill(7));
    const back = await storage.loadIdentitySeed();
    expect(back!.length).toBe(32);
    expect(back![0]).toBe(7);
  });
  test('loadSnapshots null then roundtrips', async () => {
    expect(await storage.loadSnapshots()).toBeNull();
    const payload = new TextEncoder().encode('{"labels":[]}');
    await storage.saveSnapshots(payload);
    const back = await storage.loadSnapshots();
    expect(new TextDecoder().decode(back!)).toBe('{"labels":[]}');
  });
});

describe('ObsidianStorageAdapter — sync state', () => {
  test('null when missing', async () => {
    expect(await storage.loadSyncState('aabb')).toBeNull();
  });
  test('roundtrip', async () => {
    await storage.saveSyncState('AaBb', new Uint8Array([9, 9, 9]));
    const back = await storage.loadSyncState('aabb');
    expect(Array.from(back!)).toEqual([9, 9, 9]);
  });
  test('rejects non-hex peer keys for save and load', async () => {
    await expect(storage.saveSyncState('zzz', new Uint8Array(0))).rejects.toThrow();
    await expect(storage.loadSyncState('zzz')).rejects.toThrow();
  });
});

describe('ObsidianStorageAdapter — reset semantics', () => {
  test('zero-length doc.bin is reported as null (so SDK treats as fresh)', async () => {
    await storage.saveDoc(new Uint8Array(0));
    expect(await storage.loadDoc()).toBeNull();
  });
  test('zero-length identity.seed is reported as null', async () => {
    await storage.saveIdentitySeed(new Uint8Array(0));
    expect(await storage.loadIdentitySeed()).toBeNull();
  });
  test('zero-length snapshots.json is reported as null', async () => {
    await storage.saveSnapshots(new Uint8Array(0));
    expect(await storage.loadSnapshots()).toBeNull();
  });
  test('zero-length sync state is reported as null', async () => {
    await storage.saveSyncState('aabb', new Uint8Array(0));
    expect(await storage.loadSyncState('aabb')).toBeNull();
  });
});

describe('ObsidianStorageAdapter — close + ensureDir on existing dirs', () => {
  test('close is a no-op', async () => {
    await storage.close();
    await storage.close();
  });
  test('ensureDir handles already-present directories', async () => {
    await adapter.mkdir('.obsidian');
    await storage.saveDoc(new Uint8Array([1]));
    expect(await storage.loadDoc()).not.toBeNull();
  });
  test('ensureDir handles empty path early-return', async () => {
    const s = new ObsidianStorageAdapter(adapter, '');
    // Empty root means doc/identity/etc. are written at the FS root.
    await s.saveDoc(new Uint8Array([1]));
    const back = await s.loadDoc();
    expect(back).not.toBeNull();
  });
});
