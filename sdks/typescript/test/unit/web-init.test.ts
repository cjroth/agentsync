// Exercises the explicit-init entry point (`@agentsync/sdk/web-init`). The
// rest of the test suite uses the auto-init `bundler` glue indirectly via
// `src/index.ts`; this file specifically asserts the lazy-init contract:
//   - calls before `initAgentsync()` throw a descriptive error
//   - `initAgentsync()` is idempotent (second call is a no-op)
//   - after init, the high-level Vault factory + primitives all work
//
// The wasm is fed in as raw bytes read from the freshly-built web-pkg, the
// same way an Obsidian-plugin host would feed it via an esbuild base64
// inline.

import { afterAll, beforeAll, describe, expect, test } from 'bun:test';
import { readFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  Doc,
  Identity,
  Pubkey,
  SyncState,
  Vault,
  _resetForTests,
  buildTranscript,
  contentHash,
  decodeFrame,
  defaultPort,
  encodeFrame,
  initAgentsync,
  isInitialized,
  memoryStorage,
  normalizeRendezvousUrl,
  parseAuthorizedKeys,
  randomNonce,
  renderAuthorizedKeys,
  schemaVersion,
} from '../../src/web-init.js';

const __dirname = dirname(fileURLToPath(import.meta.url));
const wasmPath = join(__dirname, '..', '..', 'dist', 'web-pkg', 'agentsync_wasm_bg.wasm');

describe('web-init: pre-initialization', () => {
  beforeAll(() => {
    _resetForTests();
  });

  test('isInitialized() is false before init', () => {
    expect(isInitialized()).toBe(false);
  });

  test('Vault.create throws a descriptive error before init', () => {
    expect(() => Vault.create({ storage: memoryStorage() })).toThrow(/WASM not initialized/);
  });

  test('Vault.open throws before init', () => {
    expect(() => Vault.open({ storage: memoryStorage() })).toThrow(/WASM not initialized/);
  });

  test('Identity.generate throws before init', () => {
    expect(() => Identity.generate()).toThrow(/WASM not initialized/);
  });

  test('Identity.fromSeed throws before init', () => {
    expect(() => Identity.fromSeed(new Uint8Array(32))).toThrow(/WASM not initialized/);
  });

  test('Pubkey.fromBytes throws before init', () => {
    expect(() => Pubkey.fromBytes(new Uint8Array(32))).toThrow(/WASM not initialized/);
  });

  test('Pubkey.fromSshString throws before init', () => {
    expect(() => Pubkey.fromSshString('ssh-ed25519 AAAA')).toThrow(/WASM not initialized/);
  });

  test('Doc.create throws before init', () => {
    expect(() => Doc.create('vault-id')).toThrow(/WASM not initialized/);
  });

  test('Doc.load throws before init', () => {
    expect(() => Doc.load(new Uint8Array(0))).toThrow(/WASM not initialized/);
  });

  test('SyncState.create throws before init', () => {
    expect(() => SyncState.create()).toThrow(/WASM not initialized/);
  });

  test('SyncState.decode throws before init', () => {
    expect(() => SyncState.decode(new Uint8Array(0))).toThrow(/WASM not initialized/);
  });

  test('top-level utility functions all throw before init', () => {
    expect(() => randomNonce()).toThrow(/WASM not initialized/);
    expect(() => contentHash(new Uint8Array([1]))).toThrow(/WASM not initialized/);
    expect(() => schemaVersion()).toThrow(/WASM not initialized/);
    expect(() => defaultPort()).toThrow(/WASM not initialized/);
    expect(() => parseAuthorizedKeys('')).toThrow(/WASM not initialized/);
    expect(() => renderAuthorizedKeys([])).toThrow(/WASM not initialized/);
    expect(() => normalizeRendezvousUrl('hub.example')).toThrow(/WASM not initialized/);
    expect(() =>
      buildTranscript(
        new Uint8Array(32),
        new Uint8Array(32),
        new Uint8Array(32),
        new Uint8Array(32),
        new Uint8Array(32),
      ),
    ).toThrow(/WASM not initialized/);
    expect(() => encodeFrame({ t: 'ping', ts: 0 })).toThrow(/WASM not initialized/);
    expect(() => decodeFrame(new Uint8Array([0]))).toThrow(/WASM not initialized/);
  });
});

describe('web-init: after initAgentsync', () => {
  beforeAll(async () => {
    _resetForTests();
    const bytes = await readFile(wasmPath);
    await initAgentsync(bytes);
  });

  afterAll(() => {
    _resetForTests();
  });

  test('isInitialized() is true', () => {
    expect(isInitialized()).toBe(true);
  });

  test('initAgentsync is idempotent — second call is a no-op', async () => {
    // Pass garbage on the second call; it must not be touched.
    await initAgentsync(new Uint8Array([0xff, 0xff]));
    expect(isInitialized()).toBe(true);
  });

  test('Vault.create + Vault.open round-trip through MemoryStorage', async () => {
    const storage = memoryStorage();
    const v1 = await Vault.create({ storage });
    await v1.writeTextFile('hello.md', '# hi\n');
    const id1 = v1.vaultIdValue();
    await v1.close();

    const v2 = await Vault.open({ storage });
    expect(v2.vaultIdValue()).toBe(id1);
    expect(await v2.readTextFile('hello.md')).toBe('# hi\n');
    await v2.close();
  });

  test('Identity / Pubkey / SyncState / Doc primitives are usable', () => {
    const id = Identity.generate();
    expect(id.seed().length).toBe(32);
    const fromSeed = Identity.fromSeed(id.seed());
    expect(fromSeed.pubkey().toSshString()).toBe(id.pubkey().toSshString());

    const ssh = id.pubkey().toSshString();
    const restored = Pubkey.fromSshString(ssh);
    expect(restored.bytes()).toEqual(id.pubkey().bytes());

    const fromBytes = Pubkey.fromBytes(id.pubkey().bytes());
    expect(fromBytes.toSshString()).toBe(ssh);

    const doc = Doc.create('00000000-0000-4000-8000-000000000001');
    doc.writeTextFile('a.md', 'hello');
    const saved = doc.save();
    const loaded = Doc.load(saved);
    expect(loaded.readFile('a.md')).toBe('hello');

    const ss = SyncState.create();
    const encoded = ss.encode();
    const decoded = SyncState.decode(encoded);
    expect(decoded.encode().length).toBeGreaterThanOrEqual(0);
  });

  test('utility re-exports operate against the initialized module', () => {
    expect(randomNonce().length).toBe(32);
    expect(contentHash(new Uint8Array([1, 2, 3]))).toMatch(/^[0-9a-f]{64}$/);
    expect(schemaVersion()).toBe(1);
    expect(defaultPort()).toBe(443);
    expect(normalizeRendezvousUrl('hub.example.com')).toContain('hub.example.com');

    // parse / render authorized keys round-trip
    const parsed = parseAuthorizedKeys('');
    expect(parsed).toEqual([]);
    const rendered = renderAuthorizedKeys([]);
    expect(typeof rendered).toBe('string');
    expect(parseAuthorizedKeys(rendered)).toEqual([]);

    // frame encode/decode round-trip
    const encoded = encodeFrame({ t: 'ping', ts: 1234 });
    const decoded = decodeFrame(encoded);
    expect(decoded.t).toBe('ping');

    // transcript: 17-byte tag + 5×32 = 177 bytes
    const t = buildTranscript(
      new Uint8Array(32),
      new Uint8Array(32),
      new Uint8Array(32),
      new Uint8Array(32),
      new Uint8Array(32),
    );
    expect(t.length).toBe(177);
  });
});
