import { describe, expect, test } from 'bun:test';
import {
  Identity,
  formatAgentsyncIdentity,
  formatPubkeySidecar,
  parseAgentsyncIdentity,
} from '../../src/index.js';

describe('agentsync-identity-v1 codec', () => {
  test('formats a 32-byte seed as the CLI does', () => {
    const seed = new Uint8Array(32).fill(7);
    const out = formatAgentsyncIdentity(seed);
    expect(out.startsWith('agentsync-identity-v1 ')).toBe(true);
    expect(out.endsWith('\n')).toBe(true);
    expect(out.includes('=')).toBe(false); // no padding
    // base64 must match the standard alphabet (cross-checked vs Node).
    const std = Buffer.from(seed).toString('base64').replace(/=+$/, '');
    expect(out).toBe(`agentsync-identity-v1 ${std}\n`);
  });

  test('round-trips an arbitrary seed', () => {
    const seed = new Uint8Array(32);
    for (let i = 0; i < 32; i++) seed[i] = (i * 37 + 11) & 0xff;
    expect(Array.from(parseAgentsyncIdentity(formatAgentsyncIdentity(seed)))).toEqual(
      Array.from(seed),
    );
  });

  test('round-trips through a real SDK identity (seed → file → Identity)', () => {
    const id = Identity.generate();
    const seed = id.seed();
    const reparsed = parseAgentsyncIdentity(formatAgentsyncIdentity(seed));
    const id2 = Identity.fromSeed(reparsed);
    expect(id2.pubkey().toSshString()).toBe(id.pubkey().toSshString());
    id.free();
    id2.free();
  });

  test('parses only the first line (trailing content ignored)', () => {
    const seed = new Uint8Array(32).fill(3);
    const body = `${formatAgentsyncIdentity(seed)}ssh-ed25519 AAAAjunk extra\n`;
    expect(Array.from(parseAgentsyncIdentity(body))).toEqual(Array.from(seed));
  });

  test('tolerates a stray padding char on decode', () => {
    const seed = new Uint8Array(32).fill(9);
    const b64 = Buffer.from(seed).toString('base64'); // keeps trailing '='
    expect(Array.from(parseAgentsyncIdentity(`agentsync-identity-v1 ${b64}\n`))).toEqual(
      Array.from(seed),
    );
  });

  test('rejects a bad prefix', () => {
    expect(() => parseAgentsyncIdentity('ssh-ed25519 AAAA\n')).toThrow(
      /not in agentsync-identity-v1 format/,
    );
  });

  test('rejects a wrong-length seed', () => {
    expect(() => parseAgentsyncIdentity('agentsync-identity-v1 QUJD\n')).toThrow(/wrong length/);
    expect(() => formatAgentsyncIdentity(new Uint8Array(16))).toThrow(/wrong length/);
  });

  test('pubkey sidecar is the ssh line plus newline', () => {
    expect(formatPubkeySidecar('ssh-ed25519 AAAAfoo')).toBe('ssh-ed25519 AAAAfoo\n');
  });
});
