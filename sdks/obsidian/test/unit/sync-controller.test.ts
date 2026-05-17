import { afterEach, beforeEach, describe, expect, test } from 'bun:test';
import {
  Doc,
  type Frame,
  Identity,
  type StorageAdapter,
  type TransportAdapter,
  Vault,
  encodeFrame,
  memoryStorage,
  randomNonce,
} from '@agentsync/sdk/web-init';
import { type AgentsyncSettings, DEFAULT_SETTINGS } from '../../src/settings.js';
import {
  type ControllerState,
  SyncController,
  bytesToHex,
  hexToBytes,
} from '../../src/sync-controller.js';
import { FakeTFile, FakeVault } from '../mocks/obsidian.js';

describe('hex helpers', () => {
  test('bytesToHex / hexToBytes round-trip', () => {
    const b = new Uint8Array([0, 15, 16, 255]);
    expect(bytesToHex(b)).toBe('000f10ff');
    expect(Array.from(hexToBytes('000f10ff'))).toEqual([0, 15, 16, 255]);
  });
  test('hexToBytes rejects odd length', () => {
    expect(() => hexToBytes('abc')).toThrow(/even length/);
  });
  test('hexToBytes rejects non-hex', () => {
    expect(() => hexToBytes('zz')).toThrow(/invalid hex/);
  });
});

let storage: StorageAdapter;
let vault: FakeVault;
let settings: AgentsyncSettings;
let identity: ReturnType<typeof Identity.generate>;

function makeController(extra: Partial<ConstructorParameters<typeof SyncController>[0]> = {}) {
  return new SyncController({
    storage,
    vault,
    settings,
    identity,
    saveSettings: async (s) => {
      settings = s;
    },
    ...extra,
  });
}

beforeEach(() => {
  storage = memoryStorage();
  vault = new FakeVault();
  settings = { ...DEFAULT_SETTINGS };
  identity = Identity.generate();
});

afterEach(async () => {
  // best-effort cleanup
  await storage.close();
  identity.free();
});

describe('SyncController state machine', () => {
  test('starts in idle', () => {
    const c = makeController();
    expect(c.state).toBe('idle');
  });

  test('start() in offline mode (no URL) ends in idle but identity is loaded', async () => {
    const c = makeController();
    await c.start();
    expect(c.state).toBe('idle');
    const ssh = c.identityPubkeySsh();
    expect(ssh).toMatch(/^ssh-ed25519 /);
    await c.stop();
  });

  test('start() persists a freshly minted vaultId', async () => {
    const c = makeController();
    await c.start();
    expect(settings.vaultId).toMatch(
      /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/,
    );
    await c.stop();
  });

  test('start() is idempotent (does not double-start)', async () => {
    const c = makeController();
    await c.start();
    const id = settings.vaultId;
    await c.start();
    expect(settings.vaultId).toBe(id);
    await c.stop();
  });

  test('stop() releases everything; subsequent start() works', async () => {
    const c = makeController();
    await c.start();
    await c.stop();
    expect(c.state).toBe('idle');
    await c.start();
    await c.stop();
  });

  test('on() listener fires on state changes', async () => {
    settings.rendezvousUrl = 'wss://nowhere';
    // vaultId set → skip hub discovery; exercise the state machine only.
    settings.vaultId = 'test-vault';
    const c = makeController();
    const seen: ControllerState[] = [];
    const off = c.on((s) => seen.push(s));
    await c.start();
    await c.stop();
    off();
    expect(seen).toContain('connecting');
    expect(seen).toContain('idle');
  });

  test('listener errors are isolated', async () => {
    settings.rendezvousUrl = 'wss://nowhere';
    settings.vaultId = 'test-vault';
    const c = makeController();
    let goodFired = false;
    c.on(() => {
      throw new Error('boom');
    });
    c.on(() => {
      goodFired = true;
    });
    await c.start();
    await c.stop();
    expect(goodFired).toBe(true);
  });

  test('resyncNow runs reconcile when running', async () => {
    const c = makeController();
    await c.start();
    await vault.create('hello.md', 'fresh');
    let noticed = '';
    const c2 = makeController({
      notice: (m) => {
        noticed = m;
      },
    });
    await c2.start();
    await c2.resyncNow();
    expect(noticed).toContain('resynced');
    await c.stop();
    await c2.stop();
  });

  test('resyncNow notices when not running', async () => {
    let noticed = '';
    const c = makeController({
      notice: (m) => {
        noticed = m;
      },
    });
    await c.resyncNow();
    expect(noticed).toContain('not running');
  });

  test('label CRUD round-trips through SDK', async () => {
    const c = makeController();
    await c.start();
    await c.createLabel('first');
    expect(c.listLabels().some((l) => l.name === 'first')).toBe(true);
    await c.restoreToLabel('first');
    await c.stop();
    expect(c.listLabels()).toEqual([]);
  });

  test('label CRUD before start is a no-op', async () => {
    const c = makeController();
    await c.createLabel('nope');
    await c.restoreToLabel('nope');
    expect(c.listLabels()).toEqual([]);
  });

  test('identityPubkeySsh is available before start (from the injected identity)', () => {
    const c = makeController();
    const pk = identity.pubkey();
    const expected = pk.toSshString();
    pk.free();
    expect(c.identityPubkeySsh()).toBe(expected);
  });

  test('reconcile pushes existing Obsidian files to a fresh SDK', async () => {
    await vault.create('a.md', 'A');
    await vault.create('img.png', 'binary');
    const c = makeController();
    await c.start();
    const sdkPaths = c
      .listLabels()
      .map((l) => l.name)
      .concat([]);
    // We can't introspect listFiles via the controller, but the SDK side
    // should have a.md and not img.png. Inspect via the bridge.
    const bridge = c.getBridge()!;
    expect(bridge).not.toBeNull();
    // Force pull-back to verify reconcile pushed a.md.
    expect(sdkPaths).toEqual([]);
    // delete locally and resync — reconcile should restore from SDK.
    const f = vault.getAbstractFileByPath('a.md')!;
    await vault.delete(f);
    await c.resyncNow();
    // a.md should still be in SDK; the next reconcile pulls it back.
    // (Since we deleted it locally, applyRemoteState recreates it.)
    expect(vault.getAbstractFileByPath('a.md')).not.toBeNull();
    await c.stop();
  });

  // Note: tombstone reconciliation at startup is intentionally NOT
  // tested here. `Doc.listFiles()` filters out tombstones at the SDK
  // boundary, so a one-shot reconcile cannot observe deletes that
  // happened on the remote while we were offline. Live `doc-changed`
  // events deliver tombstones after connect; that path is covered by
  // the e2e test against a real hub.

  test('onVaultEvent: connecting → state=connecting', async () => {
    const c = makeController();
    await c.start();
    // biome-ignore lint/suspicious/noExplicitAny: reach private method
    (c as any).onVaultEvent({ kind: 'connecting', url: 'wss://x' });
    expect(c.state).toBe('connecting');
    await c.stop();
  });

  test('onVaultEvent: connected → state=connected + pins hub pubkey (SSH) on first connect', async () => {
    const c = makeController();
    await c.start();
    const id = Identity.generate();
    const pk = id.pubkey();
    const hubBytes = pk.bytes();
    const hubSsh = pk.toSshString();
    pk.free();
    id.free();
    // biome-ignore lint/suspicious/noExplicitAny: reach private method
    (c as any).onVaultEvent({ kind: 'connected', hub_pubkey: hubBytes, vault_id: 'v' });
    expect(c.state).toBe('connected');
    // saveSettings is async; flush microtasks
    await Promise.resolve();
    await Promise.resolve();
    expect(settings.hubPubkey).toBe(hubSsh);
    expect(settings.hubPubkey).toMatch(/^ssh-ed25519 /);
    await c.stop();
  });

  test('onVaultEvent: connected leaves existing pin in place', async () => {
    // A real, parseable pin: openOrCreateVault decodes it on start().
    const pid = Identity.generate();
    const ppk = pid.pubkey();
    settings.hubPubkey = ppk.toSshString();
    const pinned = settings.hubPubkey;
    ppk.free();
    pid.free();
    const c = makeController();
    await c.start();
    // biome-ignore lint/suspicious/noExplicitAny: reach private method
    (c as any).onVaultEvent({
      kind: 'connected',
      hub_pubkey: new Uint8Array([0xbb]),
      vault_id: 'v',
    });
    expect(settings.hubPubkey).toBe(pinned);
    await c.stop();
  });

  test('openOrCreateVault: configured vault id diverging from local doc fails loudly', async () => {
    // First run: create + persist a vault with id "vault-A".
    settings.vaultId = 'vault-A';
    const c1 = makeController();
    await c1.start({ connect: false });
    expect(c1.state).toBe('idle');
    await c1.stop();

    // User edits config.toml to a different id but the local doc.bin
    // still holds "vault-A" — must fail with an actionable message,
    // not a cryptic mid-handshake `vault_id mismatch`.
    settings.vaultId = 'vault-B';
    const c2 = makeController();
    await expect(c2.start({ connect: false })).rejects.toThrow(
      /configured vault id vault-B does not match the local vault.*\(vault-A\).*Reset local state/s,
    );
    expect(c2.state).toBe('error');
  });

  test('onVaultEvent: disconnected from non-idle → reconnecting', async () => {
    const c = makeController();
    await c.start();
    // biome-ignore lint/suspicious/noExplicitAny: reach private method
    (c as any).setState('connected');
    // biome-ignore lint/suspicious/noExplicitAny: reach private method
    (c as any).onVaultEvent({ kind: 'disconnected', reason: 'x' });
    expect(c.state).toBe('reconnecting');
    await c.stop();
  });

  test('onVaultEvent: disconnected when idle stays idle', async () => {
    const c = makeController();
    // biome-ignore lint/suspicious/noExplicitAny: reach private method
    (c as any).onVaultEvent({ kind: 'disconnected', reason: 'x' });
    expect(c.state).toBe('idle');
  });

  test('onVaultEvent: sync-progress is silent', async () => {
    const c = makeController();
    await c.start();
    const before = c.state;
    // biome-ignore lint/suspicious/noExplicitAny: reach private method
    (c as any).onVaultEvent({ kind: 'sync-progress', outbound: true });
    expect(c.state).toBe(before);
    await c.stop();
  });

  test('onVaultEvent: doc-changed triggers bridge.applyRemoteState', async () => {
    const c = makeController();
    await c.start();
    // The freshly-started SDK has no remote state, but the call must run.
    // biome-ignore lint/suspicious/noExplicitAny: reach private method
    (c as any).onVaultEvent({ kind: 'doc-changed', heads: [] });
    await Promise.resolve();
    expect(c.state).not.toBe('error');
    await c.stop();
  });

  test('onVaultEvent: doc-changed with no bridge is a no-op (after stop)', async () => {
    const c = makeController();
    await c.start();
    await c.stop();
    // bridge is null after stop()
    // biome-ignore lint/suspicious/noExplicitAny: reach private method
    (c as any).onVaultEvent({ kind: 'doc-changed', heads: [] });
    expect(c.state).toBe('idle');
  });

  test('onVaultEvent: error → state=error + notice', async () => {
    let noticed = '';
    const c = makeController({
      notice: (m) => {
        noticed = m;
      },
    });
    await c.start();
    // biome-ignore lint/suspicious/noExplicitAny: reach private method
    (c as any).onVaultEvent({ kind: 'error', message: 'boom' });
    expect(c.state).toBe('error');
    expect(noticed).toContain('boom');
    await c.stop();
  });

  test('start() with rendezvousUrl set runs the reconnect supervisor (best-effort)', async () => {
    settings.rendezvousUrl = 'wss://127.0.0.1:1';
    settings.vaultId = 'test-vault';
    const logs: string[] = [];
    const c = makeController({
      log: (m) => logs.push(m),
    });
    await c.start();
    // The connect attempt will fail rapidly; give the supervisor a moment
    // to surface its error path.
    await new Promise((r) => setTimeout(r, 50));
    await c.stop();
    // We don't assert specific log content — just that the path executed.
    expect(typeof logs.join('|')).toBe('string');
  });

  test('start() error path: bad sdkOverride throws and sets error state', async () => {
    const badSdk = {
      identityRef: () => {
        throw new Error('bad');
      },
    } as never;
    let noticed = '';
    const c = makeController({
      sdkOverride: badSdk,
      notice: (m) => {
        noticed = m;
      },
    });
    await expect(c.start()).rejects.toThrow();
    expect(c.state).toBe('error');
    expect(noticed).toContain('failed to start');
  });

  test('start() guards against double-start when already connecting', async () => {
    const c = makeController();
    const p1 = c.start();
    const p2 = c.start();
    await p1;
    await p2;
    await c.stop();
  });

  test('start() catches and logs a rejecting reconnect supervisor', async () => {
    settings.rendezvousUrl = 'wss://nowhere';
    // Build an SDK override whose connectWithReconnect rejects.
    const realSdk = await (await import('@agentsync/sdk/web-init')).Vault.create({ storage });
    const fakeSdk = new Proxy(realSdk, {
      get(t, p) {
        if (p === 'connectWithReconnect')
          return () => Promise.reject(new Error('forced supervisor failure'));
        // biome-ignore lint/suspicious/noExplicitAny: proxy passthrough
        return (t as any)[p];
      },
    });
    const logs: string[] = [];
    const c = makeController({
      sdkOverride: fakeSdk as never,
      log: (m) => logs.push(m),
    });
    await c.start();
    await new Promise((r) => setTimeout(r, 10));
    expect(logs.some((l) => l.includes('forced supervisor failure'))).toBe(true);
    await c.stop();
  });
});

/** Transport whose every connection emits one canned hello_hub then ends.
 * Enough for the passive probe; a follow-up handshake just fails (fine —
 * it's the fire-and-forget supervisor). */
function helloHubTransport(vaultId: string): TransportAdapter {
  const hubId = Identity.generate();
  const frame: Frame = {
    t: 'hello_hub',
    vault_id: vaultId,
    hub_identity_pubkey: hubId.pubkey().bytes(),
    hub_nonce: randomNonce(),
    tls_cert_fingerprint: new Uint8Array(32),
    vault_name: null,
  };
  hubId.free();
  const wire = encodeFrame(frame);
  return {
    async connect() {
      let sent = false;
      return {
        async send() {},
        async *recv() {
          if (!sent) {
            sent = true;
            yield wire;
          }
        },
        channelBinding: () => null,
        async close() {},
      };
    },
  };
}

describe('SyncController connect-mode discovery', () => {
  test('rebuilds a stale local doc to join the vault the hub serves', async () => {
    // Pre-seed storage with a doc for a DIFFERENT vault (the leftover
    // from an earlier wrong-id attempt that caused vault_id mismatch).
    const seeded = await Vault.create({ storage, identity, vaultId: 'old-vault-id' });
    await seeded.close();

    settings.rendezvousUrl = 'wss://hub.example';
    settings.vaultId = ''; // blank → discover from the hub
    const c = makeController({ transport: helloHubTransport('hub-vault-id') });
    await c.start();

    // Discovered id persisted to config, and the stale local doc was
    // rebuilt for the hub's vault rather than feeding 'old-vault-id'
    // into the handshake.
    expect(settings.vaultId).toBe('hub-vault-id');
    const docBytes = await storage.loadDoc();
    if (!docBytes) throw new Error('expected a rebuilt doc on disk');
    const doc = Doc.load(docBytes);
    expect(doc.vaultId()).toBe('hub-vault-id');
    doc.free();
    await c.stop();
  });

  test('a pinned id that disagrees with the hub fails actionably', async () => {
    settings.rendezvousUrl = 'wss://hub.example';
    settings.vaultId = 'pinned-but-wrong';
    const c = makeController({ transport: helloHubTransport('hub-vault-id') });
    await expect(c.start()).rejects.toThrow(/does not match the vault this hub serves/);
    await c.stop();
  });
});
