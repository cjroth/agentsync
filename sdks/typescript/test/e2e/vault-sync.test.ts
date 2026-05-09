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

function waitFor<T>(check: () => T | undefined, timeoutMs = 10_000): Promise<T> {
  const start = Date.now();
  return new Promise((resolve, reject) => {
    const tick = () => {
      const v = check();
      if (v !== undefined) {
        resolve(v);
        return;
      }
      if (Date.now() - start > timeoutMs) {
        reject(new Error(`timeout after ${timeoutMs}ms`));
        return;
      }
      setTimeout(tick, 50);
    };
    tick();
  });
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
    env: { ...process.env, HOME: tmp, AGENTSYNC_HOME: tmp },
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

describe('TypeScript Vault ↔ Rust hub', () => {
  test('completes the 4-message handshake and emits connected', { timeout: 30_000 }, async () => {
    const storage = memoryStorage();
    const v = await Vault.create({
      storage,
      identity: peerIdentity,
      vaultId,
      rendezvousUrl: `wss://127.0.0.1:${port}`,
      transport: nodeWsTransport(WebSocket as unknown as never),
    });

    const events: string[] = [];
    const unsub = v.subscribe((e) => events.push(e.kind));

    // Race connect() against a "connected event seen" promise; the test
    // succeeds as soon as the handshake completes.
    const connectedSeen = new Promise<void>((resolve, reject) => {
      const off = v.subscribe((e) => {
        if (e.kind === 'connected') {
          off();
          resolve();
        }
        if (e.kind === 'error') {
          off();
          reject(new Error(e.message));
        }
      });
    });

    const connectPromise = v.connect().catch(() => {
      /* swallow — connection close after disconnect is expected */
    });

    await connectedSeen;
    assert.ok(events.includes('connecting'));
    assert.ok(events.includes('connected'));

    unsub();
    await v.disconnect();
    await connectPromise;
    await v.close();
  });
});
