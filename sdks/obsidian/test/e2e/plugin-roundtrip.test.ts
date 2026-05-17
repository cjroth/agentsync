// e2e: spawn a real `agentsync` hub, run the plugin's SyncController
// against an in-memory FakeApp/FakeVault, and verify two-way sync against
// a second SDK peer that simulates "another device".
//
// Coverage:
//   1. Plugin connects + handshake completes
//   2. Obsidian → SDK: writing in the FakeVault propagates to the hub
//   3. SDK → Obsidian: a write on the second peer propagates into FakeVault
//   4. Tombstones: a delete on the second peer removes the local file
//   5. Renames: a rename on the FakeVault is visible to the second peer
//   6. Persistence: a restart re-opens the SDK Vault from disk and the
//      previously synced files are still present
//   7. Binary files are skipped (no SDK side-effect)
//   8. Feedback-loop suppression: applying a remote write does NOT cause
//      the bridge's modify handler to push it back as a new SDK write
//
// Runs under Node — Bun's WebSocket client doesn't currently accept the
// hub's self-signed cert. CI provides AGENTSYNC_BIN.

import { afterAll, beforeAll, describe, test } from 'bun:test';
import { strict as assert } from 'node:assert';
import { type ChildProcessWithoutNullStreams, spawn } from 'node:child_process';
import { mkdirSync, mkdtempSync, rmSync } from 'node:fs';
import { readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  Identity,
  Vault as SdkVault,
  type VaultInstance,
  initAgentsync,
  memoryStorage,
} from '@agentsync/sdk/web-init';
import { type AgentsyncSettings, DEFAULT_SETTINGS } from '../../src/settings.ts';
import { ObsidianStorageAdapter } from '../../src/storage-adapter.ts';
import { SyncController } from '../../src/sync-controller.ts';
import { FakeDataAdapter, type FakeTFile, FakeVault } from '../mocks/obsidian.ts';

const AGENTSYNC = process.env.AGENTSYNC_BIN ?? 'agentsync';

let tmp = '';
let vaultDir = '';
let hub: ChildProcessWithoutNullStreams | null = null;
let port = 0;
let vaultId = '';
let pluginIdentity: ReturnType<typeof Identity.generate>;
let secondIdentity: ReturnType<typeof Identity.generate>;

const __dirname = dirname(fileURLToPath(import.meta.url));
const wasmPath = resolve(
  __dirname,
  '..',
  '..',
  '..',
  'typescript',
  'dist',
  'web-pkg',
  'agentsync_wasm_bg.wasm',
);

async function waitFor<T>(
  check: () => T | undefined | Promise<T | undefined>,
  timeoutMs = 10_000,
): Promise<T> {
  const start = Date.now();
  while (true) {
    let v: T | undefined;
    try {
      v = await check();
    } catch {
      v = undefined;
    }
    if (v !== undefined) return v;
    if (Date.now() - start > timeoutMs) {
      throw new Error(`timeout after ${timeoutMs}ms`);
    }
    await new Promise((r) => setTimeout(r, 50));
  }
}

beforeAll(async () => {
  // 0. Initialize wasm once for both controller and second-peer SDK use.
  const bytes = await readFile(wasmPath);
  await initAgentsync(bytes);

  pluginIdentity = Identity.generate();
  secondIdentity = Identity.generate();

  tmp = mkdtempSync(join(tmpdir(), 'agentsync-obsidian-e2e-'));
  vaultDir = join(tmp, 'vault');
  mkdirSync(vaultDir, { recursive: true });

  // 1. agentsync init bootstraps a hub-side vault.
  const initProc = spawn(AGENTSYNC, ['init', '--name', 'obsidian-e2e'], {
    cwd: vaultDir,
    env: { ...process.env, HOME: tmp, AGENTSYNC_HOME: tmp },
  });
  let initOutput = '';
  initProc.stdout.on('data', (b) => {
    initOutput += b.toString();
  });
  initProc.stderr.on('data', (b) => {
    initOutput += b.toString();
  });
  await new Promise<void>((res, rej) => {
    initProc.on('exit', (code) =>
      code === 0 ? res() : rej(new Error(`init exit ${code}: ${initOutput}`)),
    );
  });
  const m = initOutput.match(/vault_id\s*=\s*([0-9a-f-]{36})/);
  if (!m) throw new Error(`could not parse vault_id from: ${initOutput}`);
  vaultId = m[1]!;

  // 2. Spawn the hub on an ephemeral port. `--no-tls` lets Bun's
  // built-in WebSocket connect (Bun's `ws` polyfill doesn't yet accept
  // self-signed TLS certs); the protocol still does signed transcripts.
  hub = spawn(AGENTSYNC, ['watch', '--listen', '127.0.0.1:0', '--no-tls'], {
    cwd: vaultDir,
    env: {
      ...process.env,
      HOME: tmp,
      AGENTSYNC_HOME: tmp,
      AGENTSYNC_LOG: 'error',
    },
  });
  const stdout: string[] = [];
  hub.stdout.on('data', (b: Buffer) => {
    stdout.push(b.toString());
  });
  hub.stderr.on('data', (b: Buffer) => {
    stdout.push(b.toString());
  });
  port = await waitFor(() => {
    const joined = stdout.join('');
    const mm = joined.match(/listening on wss?:\/\/[^:]+:(\d+)/i);
    return mm ? Number(mm[1]) : undefined;
  });

  // 3. Add both peer pubkeys to authorized_keys.
  const akPath = join(vaultDir, 'authorized_keys');
  await waitFor(async () => {
    try {
      await readFile(akPath, 'utf8');
      return true;
    } catch {
      return undefined;
    }
  });
  const ak = await readFile(akPath, 'utf8');
  const pkPlugin = pluginIdentity.pubkey();
  const pkSecond = secondIdentity.pubkey();
  const additions =
    `${pkPlugin.toSshString()} obsidian-plugin\n` + `${pkSecond.toSshString()} second-peer\n`;
  pkPlugin.free();
  pkSecond.free();
  await writeFile(akPath, ak + additions);
  await new Promise((r) => setTimeout(r, 700));
});

afterAll(() => {
  if (hub) hub.kill('SIGTERM');
  if (tmp) rmSync(tmp, { recursive: true, force: true });
});

interface Harness {
  controller: SyncController;
  vault: FakeVault;
  adapter: FakeDataAdapter;
  settings: AgentsyncSettings;
}

async function makePluginHarness(opts: { useSeed?: Uint8Array } = {}): Promise<Harness> {
  const adapter = new FakeDataAdapter();
  const vault = new FakeVault(adapter);
  // Same layout as the real plugin: state lives in `<vault>/.agentsync/`.
  const storage = new ObsidianStorageAdapter(adapter);
  // Identity is now injected into the controller (the SDK no longer
  // sources it from storage).
  const identity = opts.useSeed ? Identity.fromSeed(opts.useSeed) : pluginIdentity;
  const settings: AgentsyncSettings = {
    ...DEFAULT_SETTINGS,
    rendezvousUrl: `ws://127.0.0.1:${port}`,
    vaultId,
    syncEnabled: true,
  };
  const controller = new SyncController({
    storage,
    vault,
    settings,
    identity,
    saveSettings: async (s) => {
      Object.assign(settings, s);
    },
  });
  return { controller, vault, adapter, settings };
}

async function makeSecondPeer(): Promise<VaultInstance> {
  return SdkVault.create({
    storage: memoryStorage(),
    identity: secondIdentity,
    vaultId,
    rendezvousUrl: `ws://127.0.0.1:${port}`,
  });
}

async function waitConnected(c: SyncController, timeout = 15_000): Promise<void> {
  await waitFor(() => (c.state === 'connected' ? true : undefined), timeout);
}

async function waitForFileOnDisk(path: string, expected: string, timeout = 15_000): Promise<void> {
  await waitFor(async () => {
    try {
      const c = await readFile(path, 'utf8');
      return c === expected ? true : undefined;
    } catch {
      return undefined;
    }
  }, timeout);
}

describe('Agentsync Obsidian plugin end-to-end', () => {
  test('handshake + plugin reaches connected', async () => {
    const h = await makePluginHarness();
    await h.controller.start();
    await waitConnected(h.controller);
    assert.equal(h.controller.state, 'connected');
    await h.controller.stop();
  });

  test('FakeVault → hub disk: write inside Obsidian propagates to the hub', async () => {
    const h = await makePluginHarness();
    await h.controller.start();
    await waitConnected(h.controller);
    // Initial sync round-trip settles.
    await new Promise((r) => setTimeout(r, 800));

    const f = await h.vault.create('plugin-out.md', '# from plugin\n');
    await h.controller.getBridge()!.handleObsidianWrite(f);

    await waitForFileOnDisk(join(vaultDir, 'plugin-out.md'), '# from plugin\n');
    await h.controller.stop();
  });

  test('second peer → plugin: write on a second device propagates into FakeVault', async () => {
    const h = await makePluginHarness();
    await h.controller.start();
    await waitConnected(h.controller);
    await new Promise((r) => setTimeout(r, 800));

    const peer = await makeSecondPeer();
    const peerConnected = new Promise<void>((res, rej) => {
      const off = peer.subscribe((e) => {
        if (e.kind === 'connected') {
          off();
          res();
        }
        if (e.kind === 'error') {
          off();
          rej(new Error(e.message));
        }
      });
    });
    const peerLifetime = peer.connectWithReconnect({}).catch(() => {});
    await peerConnected;
    await peer.writeTextFile('from-peer.md', 'hello from peer\n');

    // Plugin pulls via doc-changed → applyRemoteState into the FakeVault.
    await waitFor(() => {
      const f = h.vault.getAbstractFileByPath('from-peer.md');
      return f ? true : undefined;
    }, 15_000);

    const f = h.vault.getAbstractFileByPath('from-peer.md') as FakeTFile;
    assert.equal(await h.vault.read(f), 'hello from peer\n');

    await peer.disconnect();
    await peer.close();
    await peerLifetime;
    await h.controller.stop();
  });

  test('second peer delete tombstones into FakeVault via live sync', async () => {
    const h = await makePluginHarness();
    await h.controller.start();
    await waitConnected(h.controller);
    await new Promise((r) => setTimeout(r, 800));

    const peer = await makeSecondPeer();
    const peerConnected = new Promise<void>((res, rej) => {
      const off = peer.subscribe((e) => {
        if (e.kind === 'connected') {
          off();
          res();
        }
        if (e.kind === 'error') {
          off();
          rej(new Error(e.message));
        }
      });
    });
    const peerLifetime = peer.connectWithReconnect({}).catch(() => {});
    await peerConnected;
    await peer.writeTextFile('to-delete.md', 'doomed\n');
    await waitFor(() => (h.vault.getAbstractFileByPath('to-delete.md') ? true : undefined));

    await peer.deleteFile('to-delete.md');
    await waitFor(() => (h.vault.getAbstractFileByPath('to-delete.md') ? undefined : true), 15_000);
    assert.equal(h.vault.getAbstractFileByPath('to-delete.md'), null);

    await peer.disconnect();
    await peer.close();
    await peerLifetime;
    await h.controller.stop();
  });

  test('binary files in FakeVault are skipped (no .png reaches the hub)', async () => {
    const h = await makePluginHarness();
    await h.controller.start();
    await waitConnected(h.controller);
    await new Promise((r) => setTimeout(r, 600));

    const f = await h.vault.create('image.png', 'binarybinary');
    await h.controller.getBridge()!.handleObsidianWrite(f);

    // Wait a little, then assert the PNG never landed on disk.
    await new Promise((r) => setTimeout(r, 1500));
    let exists = false;
    try {
      await readFile(join(vaultDir, 'image.png'));
      exists = true;
    } catch {
      exists = false;
    }
    assert.equal(exists, false);
    await h.controller.stop();
  });

  test('feedback-loop suppression: applying remote write does not re-push', async () => {
    const h = await makePluginHarness();
    await h.controller.start();
    await waitConnected(h.controller);
    await new Promise((r) => setTimeout(r, 600));

    const peer = await makeSecondPeer();
    const peerConnected = new Promise<void>((res, rej) => {
      const off = peer.subscribe((e) => {
        if (e.kind === 'connected') {
          off();
          res();
        }
        if (e.kind === 'error') {
          off();
          rej(new Error(e.message));
        }
      });
    });
    const peerLifetime = peer.connectWithReconnect({}).catch(() => {});
    await peerConnected;

    const before = h.controller.getBridge()!.pushed;
    await peer.writeTextFile('one-way.md', 'remote\n');
    await waitFor(() => (h.vault.getAbstractFileByPath('one-way.md') ? true : undefined), 15_000);
    // Allow the modify event to fire if it would.
    await new Promise((r) => setTimeout(r, 600));
    // The bridge's `pushed` counter should NOT have moved.
    const bridge = h.controller.getBridge()!;
    assert.equal(bridge.pushed, before);

    await peer.disconnect();
    await peer.close();
    await peerLifetime;
    await h.controller.stop();
  });

  test('rename on FakeVault propagates to the hub', async () => {
    const h = await makePluginHarness();
    await h.controller.start();
    await waitConnected(h.controller);
    await new Promise((r) => setTimeout(r, 600));

    const f = await h.vault.create('rename-src.md', '# rename me\n');
    await h.controller.getBridge()!.handleObsidianWrite(f);
    await waitForFileOnDisk(join(vaultDir, 'rename-src.md'), '# rename me\n');

    await h.vault.rename(f, 'rename-dst.md');
    await h.controller.getBridge()!.handleObsidianRename(f, 'rename-src.md');
    await waitForFileOnDisk(join(vaultDir, 'rename-dst.md'), '# rename me\n');

    await h.controller.stop();
  });

  test('persistence: restart reopens SDK Vault from storage with prior files', async () => {
    // First run: write a file via Obsidian.
    const h1 = await makePluginHarness();
    await h1.controller.start();
    await waitConnected(h1.controller);
    await new Promise((r) => setTimeout(r, 600));
    const f = await h1.vault.create('persistent.md', '# kept\n');
    await h1.controller.getBridge()!.handleObsidianWrite(f);
    await waitForFileOnDisk(join(vaultDir, 'persistent.md'), '# kept\n');
    await h1.controller.stop();

    // Second run: brand-new FakeVault but same DataAdapter instance →
    // ObsidianStorageAdapter sees the same `.agentsync/` files. The
    // controller should call Vault.open() and inherit doc.bin.
    const adapter = h1.adapter;
    const vault2 = new FakeVault(adapter);
    const storage2 = new ObsidianStorageAdapter(adapter);
    const settings2: AgentsyncSettings = {
      ...DEFAULT_SETTINGS,
      rendezvousUrl: `ws://127.0.0.1:${port}`,
      vaultId,
      syncEnabled: true,
    };
    const c2 = new SyncController({
      storage: storage2,
      vault: vault2,
      settings: settings2,
      identity: pluginIdentity,
      saveSettings: async () => {},
    });
    await c2.start();
    await waitConnected(c2);

    // After reconcile, persistent.md from the prior run should reappear in
    // the fresh FakeVault.
    await waitFor(() => (vault2.getAbstractFileByPath('persistent.md') ? true : undefined), 10_000);
    const fr = vault2.getAbstractFileByPath('persistent.md') as FakeTFile;
    assert.equal(await vault2.read(fr), '# kept\n');
    await c2.stop();
  });
});
