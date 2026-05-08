// e2e: spawn a real `agentsync` hub and verify the wasm SDK can speak its
// wire protocol. The hub picks an ephemeral port; we open a WSS connection
// (with cert validation disabled — TLS trust is bound at the application
// layer via the cert fingerprint inside the handshake transcript), wait
// for the first frame, and assert it decodes as a `hello_hub` carrying
// the hub's vault id.
//
// This test runs under Node (not Bun) because Bun's built-in WebSocket
// client doesn't currently support the hub's ed25519 self-signed TLS cert.
// `bun run test:e2e` invokes node with --experimental-strip-types so this
// .ts file can run unmodified.

import { strict as assert } from 'node:assert';
import { type ChildProcessWithoutNullStreams, spawn } from 'node:child_process';
import { mkdirSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { after, before, describe, test } from 'node:test';
import { WebSocket } from 'ws';
// Import the built SDK so this test exercises the same artifact npm
// consumers will install. `bun run build` (or just `build:ts`) must run
// first; the test:e2e script in package.json wires that up.
import { decodeFrame } from '../../dist/index.js';

const AGENTSYNC = process.env.AGENTSYNC_BIN ?? 'agentsync';

let tmp: string;
let vaultDir: string;
let hub: ChildProcessWithoutNullStreams | null = null;
let port = 0;

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
  tmp = mkdtempSync(join(tmpdir(), 'agentsync-e2e-'));
  vaultDir = join(tmp, 'vault');
  mkdirSync(vaultDir, { recursive: true });

  const initProc = spawn(AGENTSYNC, ['init', '--name', 'wasm-e2e'], {
    cwd: vaultDir,
    env: { ...process.env, HOME: tmp, AGENTSYNC_HOME: tmp },
  });
  await new Promise<void>((res, rej) => {
    initProc.on('exit', (code) => (code === 0 ? res() : rej(new Error(`init exit ${code}`))));
  });

  hub = spawn(AGENTSYNC, ['--listen', '127.0.0.1:0'], {
    cwd: vaultDir,
    env: { ...process.env, HOME: tmp, AGENTSYNC_HOME: tmp },
  });

  // The CLI announces the bound port on stdout: `listening on wss://addr:port`.
  const stdout: string[] = [];
  hub.stdout.on('data', (b: Buffer) => {
    stdout.push(b.toString());
  });

  port = await waitFor(() => {
    const joined = stdout.join('');
    const m = joined.match(/listening on wss:\/\/[^:]+:(\d+)/i);
    return m ? Number(m[1]) : undefined;
  });
});

after(() => {
  if (hub) hub.kill('SIGTERM');
  if (tmp) rmSync(tmp, { recursive: true, force: true });
});

describe('wasm SDK ↔ agentsync hub', () => {
  test('decodes the hello_hub frame the hub puts on the wire', async () => {
    const ws = new WebSocket(`wss://127.0.0.1:${port}`, {
      rejectUnauthorized: false,
    });

    const firstFrame = await new Promise<Uint8Array>((resolve, reject) => {
      ws.on('error', reject);
      ws.on('message', (data: Buffer) => {
        resolve(new Uint8Array(data));
      });
      setTimeout(() => reject(new Error('no frame within 5s')), 5_000);
    });

    ws.close();

    const frame = decodeFrame(firstFrame);
    assert.equal(frame.t, 'hello_hub');
    if (frame.t === 'hello_hub') {
      assert.equal(typeof frame.vault_id, 'string');
      assert.ok(frame.vault_id.length > 0);
      assert.equal(frame.hub_identity_pubkey.length, 32);
      assert.equal(frame.hub_nonce.length, 32);
      // Phase 2+ fingerprint is non-empty when WSS is on.
      assert.equal(frame.tls_cert_fingerprint.length, 32);
    }
  });
});
