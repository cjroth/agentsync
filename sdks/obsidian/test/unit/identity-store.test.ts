import { describe, expect, test } from 'bun:test';
import { type IdentityIO, loadIdentity, loadOrCreateIdentity } from '../../src/identity-store.js';

/** In-memory IdentityIO recording what was written. */
class FakeIO implements IdentityIO {
  body: string | null = null;
  pub: string | null = null;
  writes = 0;
  constructor(seed?: string) {
    this.body = seed ?? null;
  }
  async read(): Promise<string | null> {
    return this.body;
  }
  async write(body: string, pub: string): Promise<void> {
    this.body = body;
    this.pub = pub;
    this.writes += 1;
  }
  describe(): string {
    return '<fake>';
  }
}

describe('loadIdentity', () => {
  test('returns null when no identity on disk (never creates)', async () => {
    const io = new FakeIO();
    expect(await loadIdentity(io)).toBeNull();
    expect(io.writes).toBe(0);
  });

  test('loads an existing identity', async () => {
    const io = new FakeIO();
    const { identity } = await loadOrCreateIdentity(io); // seed the file
    const ssh = identity.pubkey().toSshString();
    identity.free();

    const loaded = await loadIdentity(new FakeIO(io.body ?? undefined));
    expect(loaded).not.toBeNull();
    expect(loaded?.pubkey().toSshString()).toBe(ssh);
    loaded?.free();
  });
});

describe('loadOrCreateIdentity', () => {
  test('generates + persists a fresh identity with a .pub sidecar', async () => {
    const io = new FakeIO();
    const { identity, created } = await loadOrCreateIdentity(io);
    expect(created).toBe(true);
    expect(io.writes).toBe(1);
    expect(io.body?.startsWith('agentsync-identity-v1 ')).toBe(true);
    expect(io.pub).toBe(`${identity.pubkey().toSshString()}\n`);
    identity.free();
  });

  test('reuses an existing identity (does not rewrite)', async () => {
    const io = new FakeIO();
    const first = await loadOrCreateIdentity(io);
    const firstSsh = first.identity.pubkey().toSshString();
    first.identity.free();

    const second = await loadOrCreateIdentity(io);
    expect(second.created).toBe(false);
    expect(io.writes).toBe(1); // unchanged — no second write
    expect(second.identity.pubkey().toSshString()).toBe(firstSsh);
    second.identity.free();
  });
});
