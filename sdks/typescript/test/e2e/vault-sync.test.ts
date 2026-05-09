// e2e: spawn a real `agentsync` hub, wire up a TS-side `Vault` peer, and
// verify that:
//   1. The 4-message handshake completes
//   2. The Vault emits a `connected` event
//   3. The doc syncs (TS peer learns whatever files the hub has)
//   4. Disconnect tears down cleanly
//
// This exercises the full TypeScript SDK protocol stack: WebSocket
// transport, frame codec, transcript signing, Automerge incremental sync,
// storage persistence (memory adapter for the test).
//
// Runs under Node — Bun's WebSocket client doesn't currently accept the
// hub's ed25519 self-signed cert. CI provides AGENTSYNC_BIN.

import { strict as assert } from 'node:assert';
import { type ChildProcessWithoutNullStreams, spawn } from 'node:child_process';
import { mkdirSync, mkdtempSync, rmSync } from 'node:fs';
import { readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { after, before, describe, test } from 'node:test';
import { WebSocket } from 'ws';
import { Identity, Vault, memoryStorage, nodeWsTransport } from '../../dist/index.js';

const AGENTSYNC = process.env.AGENTSYNC_BIN ?? 'agentsync';

let tmp = '';
let vaultDir = '';
let hub: ChildProcessWithoutNullStreams | null = null;
let port = 0;
let vaultId = '';
let peerIdentity: ReturnType<typeof Identity.generate>;

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

before(async () => {
  tmp = mkdtempSync(join(tmpdir(), 'agentsync-vault-e2e-'));
  vaultDir = join(tmp, 'vault');
  mkdirSync(vaultDir, { recursive: true });

  // 1. Run agentsync init to bootstrap the hub vault.
  const initProc = spawn(AGENTSYNC, ['init', '--name', 'vault-e2e'], {
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

  // 2. Pull the vault_id out of the printed init output.
  const vaultIdMatch = initOutput.match(/vault_id\s*=\s*([0-9a-f-]{36})/);
  if (!vaultIdMatch) throw new Error(`could not parse vault_id from: ${initOutput}`);
  vaultId = vaultIdMatch[1]!;

  // 3. Generate a TS-side identity for use later when adding it to authorized_keys.
  peerIdentity = Identity.generate();

  // 4. Spawn the hub. The materializer writes `authorized_keys` to the
  // vault root once it's running.
  hub = spawn(AGENTSYNC, ['--listen', '127.0.0.1:0'], {
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

  // 5. Pull the bound port out of the announce line.
  port = await waitFor(() => {
    const joined = stdout.join('');
    const m = joined.match(/listening on wss:\/\/[^:]+:(\d+)/i);
    return m ? Number(m[1]) : undefined;
  });

  // 6. Wait for the hub to materialize `authorized_keys`, then append our
  // pubkey. The hub re-ingests on file change, picks up the new pubkey,
  // and accepts handshakes signed by it.
  const akPath = join(vaultDir, 'authorized_keys');
  await waitFor(async () => {
    try {
      await readFile(akPath, 'utf8');
      return true;
    } catch {
      return undefined;
    }
  });
  const akContent = await readFile(akPath, 'utf8');
  const peerPk = peerIdentity.pubkey();
  const peerSshLine = `${peerPk.toSshString()} ts-e2e-peer\n`;
  peerPk.free();
  await writeFile(akPath, akContent + peerSshLine);
  // Give the hub a brief window to ingest + recompute the authorized set.
  await new Promise((r) => setTimeout(r, 500));
});

after(() => {
  if (hub) hub.kill('SIGTERM');
  if (tmp) rmSync(tmp, { recursive: true, force: true });
});

async function makeTsVault() {
  const storage = memoryStorage();
  const v = await Vault.create({
    storage,
    identity: peerIdentity,
    vaultId,
    rendezvousUrl: `wss://127.0.0.1:${port}`,
    transport: nodeWsTransport(WebSocket as unknown as never),
  });
  return { v, storage };
}

/** Wait until `v.subscribe` emits an event whose `kind` is in `wanted`. */
function waitForEvent(v: Awaited<ReturnType<typeof makeTsVault>>['v'], wanted: string[]) {
  return new Promise<void>((resolve, reject) => {
    const off = v.subscribe((e) => {
      if (wanted.includes(e.kind)) {
        off();
        resolve();
      }
      if (e.kind === 'error') {
        off();
        reject(new Error(e.message));
      }
    });
  });
}

describe('TypeScript Vault ↔ Rust hub', () => {
  test('completes the 4-message handshake and emits connected', { timeout: 30_000 }, async () => {
    const { v } = await makeTsVault();
    const events: string[] = [];
    const unsub = v.subscribe((e) => events.push(e.kind));
    const connected = waitForEvent(v, ['connected']);
    const connectPromise = v.connect().catch(() => {});
    await connected;
    assert.ok(events.includes('connecting'));
    assert.ok(events.includes('connected'));
    unsub();
    await v.disconnect();
    await connectPromise;
    await v.close();
  });

  test('TS write propagates to hub disk', { timeout: 30_000 }, async () => {
    const { v } = await makeTsVault();
    const connected = waitForEvent(v, ['connected']);
    const connectPromise = v.connect().catch(() => {});
    await connected;
    // Wait briefly for the initial sync round-trip to settle so the TS
    // doc and hub doc share heads before we add a new local change.
    await new Promise((r) => setTimeout(r, 800));

    await v.writeTextFile('hello-from-ts.md', '# from ts\n');

    const target = join(vaultDir, 'hello-from-ts.md');
    const content = await waitFor(async () => {
      try {
        const c = await readFile(target, 'utf8');
        return c === '# from ts\n' ? c : undefined;
      } catch {
        return undefined;
      }
    }, 15_000);
    assert.equal(content, '# from ts\n');

    await v.disconnect();
    await connectPromise;
    await v.close();
  });

  test('hub write propagates to TS Vault', { timeout: 30_000 }, async () => {
    const { v } = await makeTsVault();
    const connected = waitForEvent(v, ['connected']);
    const connectPromise = v.connect().catch(() => {});
    await connected;

    // Drop a file into the hub's vault directory; the materializer
    // ingest loop picks it up and the sync engine forwards it to peers.
    await writeFile(join(vaultDir, 'hello-from-hub.md'), '# from hub\n');

    // Poll the TS Vault until the file shows up.
    const text = await waitFor(() => {
      try {
        return v.readTextFile('hello-from-hub.md');
      } catch {
        return undefined;
      }
    }, 15_000);
    assert.equal(text, '# from hub\n');

    await v.disconnect();
    await connectPromise;
    await v.close();
  });

  test('reconnect after hub restart', { timeout: 60_000 }, async () => {
    const { v } = await makeTsVault();
    const connected1 = waitForEvent(v, ['connected']);
    const reconnectAbort = v.connectWithReconnect({ initialBackoffMs: 200 });
    // Don't await — connectWithReconnect runs forever.
    reconnectAbort.catch(() => {});
    await connected1;

    // Kill the hub and re-spawn on the SAME port so the TS Vault's
    // backoff loop reconnects to the new instance.
    hub!.kill('SIGTERM');
    await new Promise<void>((r) => hub!.once('exit', () => r()));

    const newHub = spawn(AGENTSYNC, ['--listen', `127.0.0.1:${port}`], {
      cwd: vaultDir,
      env: { ...process.env, HOME: tmp, AGENTSYNC_HOME: tmp },
    });
    newHub.stdout.on('data', () => {});
    newHub.stderr.on('data', () => {});
    hub = newHub;

    // Subscribe AFTER killing so we only catch the re-connect event.
    const reconnected = new Promise<void>((resolve, reject) => {
      const t0 = Date.now();
      const tick = setInterval(() => {
        if (v.isConnected()) {
          clearInterval(tick);
          resolve();
        } else if (Date.now() - t0 > 30_000) {
          clearInterval(tick);
          reject(new Error('reconnect timeout'));
        }
      }, 100);
    });
    await reconnected;
    assert.equal(v.isConnected(), true);

    await v.disconnect();
    await v.close();
  });
});
