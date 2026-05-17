import { afterEach, beforeEach, describe, expect, test } from 'bun:test';
import { Vault, type VaultInstance, memoryStorage } from '@agentsync/sdk/web-init';
import { ObsidianVaultBridge } from '../../src/bridge.js';
import { shouldSync } from '../../src/path-filter.js';
import { type FakeTAbstractFile, FakeTFile, FakeVault } from '../mocks/obsidian.js';

let sdk: VaultInstance;
let vault: FakeVault;
let bridge: ObsidianVaultBridge;
let logged: string[];

function makeBridge(opts: { ignoreGlobs?: string[] } = {}) {
  logged = [];
  bridge = new ObsidianVaultBridge({
    vault,
    sdk,
    filter: (p) => shouldSync(p, opts.ignoreGlobs ?? []),
    log: (m) => logged.push(m),
  });
  return bridge;
}

beforeEach(async () => {
  vault = new FakeVault();
  sdk = await Vault.create({ storage: memoryStorage() });
  makeBridge();
});

afterEach(async () => {
  await sdk.close();
});

describe('default isFile predicate', () => {
  test('uses TFile.extension to discriminate when no override is passed', async () => {
    // Don't supply `isFile` — exercise the default discriminator.
    const b = new ObsidianVaultBridge({
      vault,
      sdk,
      filter: () => true,
    });
    const tfile = await vault.create('hello.md', 'hi');
    await b.handleObsidianWrite(tfile);
    expect(sdk.fileExists('hello.md')).toBe(true);
    // Folder-shaped object (no extension getter) is rejected.
    const folderShape = { path: 'Drafts', name: 'Drafts' } as FakeTAbstractFile;
    await b.handleObsidianWrite(folderShape);
    expect(sdk.fileExists('Drafts')).toBe(false);
    // A null/undefined file is also handled by the default predicate.
    // biome-ignore lint/suspicious/noExplicitAny: covering the null branch
    await b.handleObsidianWrite(null as any);
  });
});

describe('handleObsidianWrite', () => {
  test('pushes a text file to the SDK on create/modify', async () => {
    const f = await vault.create('a.md', 'hello');
    await bridge.handleObsidianWrite(f);
    expect(sdk.fileExists('a.md')).toBe(true);
    expect(await sdk.readTextFile('a.md')).toBe('hello');
    expect(bridge.pushed).toBe(1);
  });

  test('skips folder-typed events (no extension)', async () => {
    // FakeTFile-only — passing a TFolder-shaped object should bail.
    const folder = { path: 'Drafts', name: 'Drafts' } as FakeTAbstractFile;
    await bridge.handleObsidianWrite(folder);
    expect(bridge.pushed).toBe(0);
  });

  test('skips paths the filter rejects (binary)', async () => {
    const f = await vault.create('img.png', 'binarydata');
    await bridge.handleObsidianWrite(f);
    expect(sdk.fileExists('img.png')).toBe(false);
    expect(bridge.skipped).toBe(1);
  });

  test('skips when content is already equal (loop short-circuit)', async () => {
    const f = await vault.create('a.md', 'same');
    await sdk.writeTextFile('a.md', 'same');
    await bridge.handleObsidianWrite(f);
    // No additional push — pushed counter stays at 0.
    expect(bridge.pushed).toBe(0);
  });

  test('respects suppression — consumes one token then bails', async () => {
    const f = await vault.create('a.md', 'hello');
    bridge.suppress('a.md');
    await bridge.handleObsidianWrite(f);
    expect(sdk.fileExists('a.md')).toBe(false);
    // Subsequent event without suppression goes through.
    await bridge.handleObsidianWrite(f);
    expect(sdk.fileExists('a.md')).toBe(true);
  });

  test('suppression token count > 1 decrements rather than deletes', async () => {
    const f = await vault.create('a.md', 'hello');
    bridge.suppress('a.md');
    bridge.suppress('a.md');
    await bridge.handleObsidianWrite(f);
    await bridge.handleObsidianWrite(f);
    expect(sdk.fileExists('a.md')).toBe(false);
    await bridge.handleObsidianWrite(f);
    expect(sdk.fileExists('a.md')).toBe(true);
  });
});

describe('handleObsidianDelete', () => {
  test('deletes from SDK when known', async () => {
    await sdk.writeTextFile('a.md', 'hi');
    const f = new FakeTFile('a.md');
    await bridge.handleObsidianDelete(f);
    expect(sdk.fileExists('a.md')).toBe(false);
    expect(bridge.pushed).toBe(1);
  });

  test('no-ops when SDK does not know the path', async () => {
    const f = new FakeTFile('never.md');
    await bridge.handleObsidianDelete(f);
    expect(bridge.pushed).toBe(0);
  });

  test('respects suppression', async () => {
    await sdk.writeTextFile('a.md', 'hi');
    const f = new FakeTFile('a.md');
    bridge.suppress('a.md');
    await bridge.handleObsidianDelete(f);
    expect(sdk.fileExists('a.md')).toBe(true);
  });
});

describe('handleObsidianRename', () => {
  test('rename within filter scope renames in SDK', async () => {
    await sdk.writeTextFile('old.md', 'x');
    const f = new FakeTFile('new.md');
    await bridge.handleObsidianRename(f, 'old.md');
    expect(sdk.fileExists('old.md')).toBe(false);
    expect(sdk.fileExists('new.md')).toBe(true);
    expect(bridge.pushed).toBe(1);
  });

  test('rename when SDK never knew old path falls back to write', async () => {
    const tfile = await vault.create('new.md', 'fresh');
    await bridge.handleObsidianRename(tfile, 'old.md');
    expect(sdk.fileExists('new.md')).toBe(true);
    expect(await sdk.readTextFile('new.md')).toBe('fresh');
  });

  test('rename out of filter scope deletes from SDK', async () => {
    await sdk.writeTextFile('old.md', 'x');
    const f = new FakeTFile('old.png');
    await bridge.handleObsidianRename(f, 'old.md');
    expect(sdk.fileExists('old.md')).toBe(false);
  });

  test('rename out-of-scope when SDK didn’t know is a no-op', async () => {
    const f = new FakeTFile('old.png');
    await bridge.handleObsidianRename(f, 'never.md');
    expect(bridge.pushed).toBe(0);
  });

  test('rename into filter scope writes the file', async () => {
    const tfile = await vault.create('new.md', 'fresh');
    await bridge.handleObsidianRename(tfile, 'old.png');
    expect(sdk.fileExists('new.md')).toBe(true);
    expect(await sdk.readTextFile('new.md')).toBe('fresh');
  });

  test('rename neither side allowed → no-op', async () => {
    const f = new FakeTFile('img2.png');
    await bridge.handleObsidianRename(f, 'img1.png');
    expect(bridge.pushed).toBe(0);
  });

  test('rename respects suppression', async () => {
    await sdk.writeTextFile('old.md', 'x');
    const f = new FakeTFile('new.md');
    bridge.suppress('new.md');
    await bridge.handleObsidianRename(f, 'old.md');
    expect(sdk.fileExists('old.md')).toBe(true);
    expect(sdk.fileExists('new.md')).toBe(false);
  });
});

describe('applyOneRemoteFile', () => {
  test('creates file in Obsidian when missing', async () => {
    await sdk.writeTextFile('Notes/x.md', 'hello');
    const meta = sdk.listFiles().find((m) => m.path === 'Notes/x.md')!;
    await bridge.applyOneRemoteFile(meta);
    const f = vault.getAbstractFileByPath('Notes/x.md');
    expect(f).not.toBeNull();
    expect(await vault.read(f as FakeTFile)).toBe('hello');
    expect(bridge.pulled).toBe(1);
  });

  test('modifies file when content differs', async () => {
    await vault.create('a.md', 'old');
    await sdk.writeTextFile('a.md', 'new');
    const meta = sdk.listFiles().find((m) => m.path === 'a.md')!;
    await bridge.applyOneRemoteFile(meta);
    const f = vault.getAbstractFileByPath('a.md') as FakeTFile;
    expect(await vault.read(f)).toBe('new');
  });

  test('skips when content is already equal', async () => {
    await vault.create('a.md', 'same');
    await sdk.writeTextFile('a.md', 'same');
    const meta = sdk.listFiles().find((m) => m.path === 'a.md')!;
    await bridge.applyOneRemoteFile(meta);
    expect(bridge.pulled).toBe(0);
  });

  test('skips kind != Text', async () => {
    const meta = {
      id: 'fake',
      path: 'img.png',
      kind: 'Attachment' as const,
      size: 1,
      created_at: 0,
      updated_at: 0,
    };
    await bridge.applyOneRemoteFile(meta);
    expect(bridge.pulled).toBe(0);
  });

  test('skips when filter rejects', async () => {
    const meta = {
      id: 'fake',
      path: 'authorized_keys',
      kind: 'Text' as const,
      size: 1,
      created_at: 0,
      updated_at: 0,
    };
    await bridge.applyOneRemoteFile(meta);
    expect(bridge.pulled).toBe(0);
  });

  test('deletes from Obsidian when SDK has tombstone', async () => {
    await vault.create('a.md', 'doomed');
    const meta = {
      id: 'fake',
      path: 'a.md',
      kind: 'Text' as const,
      size: 0,
      created_at: 0,
      updated_at: 0,
      deleted_at: Date.now(),
    };
    await bridge.applyOneRemoteFile(meta);
    expect(vault.getAbstractFileByPath('a.md')).toBeNull();
    expect(bridge.pulled).toBe(1);
  });

  test('tombstone with no local file is a no-op', async () => {
    const meta = {
      id: 'fake',
      path: 'never.md',
      kind: 'Text' as const,
      size: 0,
      created_at: 0,
      updated_at: 0,
      deleted_at: Date.now(),
    };
    await bridge.applyOneRemoteFile(meta);
    expect(bridge.pulled).toBe(0);
  });
});

describe('applyRemoteState + ensureFolderFor', () => {
  test('creates parent folders before file', async () => {
    await sdk.writeTextFile('a/b/c.md', 'deep');
    await bridge.applyRemoteState();
    expect(vault.getAbstractFileByPath('a')).not.toBeNull();
    expect(vault.getAbstractFileByPath('a/b')).not.toBeNull();
    expect(vault.getAbstractFileByPath('a/b/c.md')).not.toBeNull();
  });

  test('flat path needs no folder creation', async () => {
    await sdk.writeTextFile('flat.md', 'x');
    await bridge.applyRemoteState();
    expect(vault.getAbstractFileByPath('flat.md')).not.toBeNull();
  });

  test('existing folder is reused', async () => {
    await vault.createFolder('a');
    await sdk.writeTextFile('a/b.md', 'hi');
    await bridge.applyRemoteState();
    expect(vault.getAbstractFileByPath('a/b.md')).not.toBeNull();
  });

  // Regression: on reopen, Obsidian's metadata cache is cold so
  // getAbstractFileByPath returns null for folders that physically exist
  // (from a prior session's sync), and createFolder throws "Folder
  // already exists." That race must be tolerated; real failures must not.
  test('tolerates cold-cache "Folder already exists." race', async () => {
    let calls = 0;
    const stub = {
      getFiles: () => [],
      getAbstractFileByPath: () => null, // cold cache: everything looks absent
      read: async () => '',
      create: async () => ({}) as never,
      modify: async () => {},
      delete: async () => {},
      rename: async () => {},
      createFolder: async () => {
        calls += 1;
        throw new Error('Folder already exists.');
      },
    };
    const b = new ObsidianVaultBridge({ vault: stub as never, sdk, filter: () => true });
    await b.ensureFolderFor('a/b/c.md'); // must not throw
    expect(calls).toBeGreaterThan(0);
  });

  test('still propagates a non-"already exists" createFolder failure', async () => {
    const stub = {
      getFiles: () => [],
      getAbstractFileByPath: () => null,
      read: async () => '',
      create: async () => ({}) as never,
      modify: async () => {},
      delete: async () => {},
      rename: async () => {},
      createFolder: async () => {
        throw new Error('EACCES: permission denied');
      },
    };
    const b = new ObsidianVaultBridge({ vault: stub as never, sdk, filter: () => true });
    await expect(b.ensureFolderFor('x/y.md')).rejects.toThrow(/permission denied/);
  });
});

describe('applyOneRemoteFile — cold metadata cache race', () => {
  test('recovers from "File already exists." by writing remote content', async () => {
    await sdk.writeTextFile('note.md', 'remote-content');
    const meta = sdk.listFiles().find((m) => m.path === 'note.md')!;
    let getCalls = 0;
    const modifiedWith: string[] = [];
    const stub = {
      getFiles: () => [],
      getAbstractFileByPath: (p: string) => {
        if (p !== 'note.md') return null;
        getCalls += 1;
        // Cold on the first (existence) check; resolves once Obsidian
        // notices the file after the failed create.
        return getCalls === 1 ? null : new FakeTFile('note.md');
      },
      read: async () => '',
      create: async () => {
        throw new Error('File already exists.');
      },
      modify: async (_f: unknown, data: string) => {
        modifiedWith.push(data);
      },
      delete: async () => {},
      rename: async () => {},
      createFolder: async () => {},
    };
    const b = new ObsidianVaultBridge({ vault: stub as never, sdk, filter: () => true });
    await b.applyOneRemoteFile(meta); // must not throw
    expect(modifiedWith).toEqual(['remote-content']);
  });

  test('still propagates a non-"already exists" create failure', async () => {
    await sdk.writeTextFile('boom.md', 'x');
    const meta = sdk.listFiles().find((m) => m.path === 'boom.md')!;
    const stub = {
      getFiles: () => [],
      getAbstractFileByPath: () => null,
      read: async () => '',
      create: async () => {
        throw new Error('ENOSPC: no space left on device');
      },
      modify: async () => {},
      delete: async () => {},
      rename: async () => {},
      createFolder: async () => {},
    };
    const b = new ObsidianVaultBridge({ vault: stub as never, sdk, filter: () => true });
    await expect(b.applyOneRemoteFile(meta)).rejects.toThrow(/no space left/);
  });
});
