import { describe, expect, test } from 'bun:test';
import { Identity, Pubkey, randomNonce } from '../../src/index.js';

describe('Identity', () => {
  test('generate produces a unique seed each call', () => {
    const a = Identity.generate();
    const b = Identity.generate();
    expect(a.seed()).not.toEqual(b.seed());
    expect(a.seed().length).toBe(32);
  });

  test('fromSeed round-trips through pubkey', () => {
    const id = Identity.generate();
    const seed = id.seed();
    const restored = Identity.fromSeed(seed);
    expect(restored.pubkey().toSshString()).toBe(id.pubkey().toSshString());
  });

  test('fromSeed rejects wrong length', () => {
    expect(() => Identity.fromSeed(new Uint8Array(31))).toThrow();
  });

  test('sign produces a 64-byte signature that the pubkey verifies', async () => {
    const id = Identity.generate();
    const msg = new TextEncoder().encode('hello agentsync');
    const sig = await id.sign(msg);
    expect(sig.length).toBe(64);
    expect(id.pubkey().verify(msg, sig)).toBe(true);
    expect(id.pubkey().verify(new TextEncoder().encode('hello agentsync!'), sig)).toBe(false);
  });

  test('pubkey ssh string round-trips', () => {
    const id = Identity.generate();
    const ssh = id.pubkey().toSshString();
    expect(ssh.startsWith('ssh-ed25519 ')).toBe(true);
    const restored = Pubkey.fromSshString(ssh);
    expect(restored.bytes()).toEqual(id.pubkey().bytes());
  });

  test('fingerprint matches OpenSSH SHA256:<base64> shape', () => {
    const fp = Identity.generate().pubkey().fingerprint();
    expect(fp).toMatch(/^SHA256:[A-Za-z0-9+/]{43}$/);
  });
});

describe('randomNonce', () => {
  test('returns 32 unique bytes per call', () => {
    const a = randomNonce();
    const b = randomNonce();
    expect(a.length).toBe(32);
    expect(a).not.toEqual(b);
  });
});
