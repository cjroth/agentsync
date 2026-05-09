// Sanity check: two Doc instances + their SyncStates can converge via the
// generateSyncMessage / receiveSyncMessage primitives. Validates the
// wasm bindings without involving the wire protocol.

import { describe, expect, it } from 'bun:test';
import { Doc, SyncState } from '../../src/index.js';

function syncFully(a: Doc, aState: SyncState, b: Doc, bState: SyncState) {
  for (let i = 0; i < 50; i++) {
    const m1 = a.generateSyncMessage(aState);
    if (m1) b.receiveSyncMessage(bState, m1);
    const m2 = b.generateSyncMessage(bState);
    if (m2) a.receiveSyncMessage(aState, m2);
    if (!m1 && !m2) return;
  }
  throw new Error('sync did not converge in 50 rounds');
}

describe('Doc sync round-trip', () => {
  it('two docs with the same vault_id converge to identical files', () => {
    const vid = '11111111-1111-4111-8111-111111111111';
    const a = new Doc(vid);
    const b = new Doc(vid);

    a.writeTextFile('a-only.md', 'from A');
    b.writeTextFile('b-only.md', 'from B');

    const aState = new SyncState();
    const bState = new SyncState();
    syncFully(a, aState, b, bState);

    const aPaths = a
      .listFiles()
      .filter((f) => !f.deleted_at)
      .map((f) => f.path)
      .sort();
    const bPaths = b
      .listFiles()
      .filter((f) => !f.deleted_at)
      .map((f) => f.path)
      .sort();
    expect(aPaths).toEqual(bPaths);
    expect(aPaths).toContain('a-only.md');
    expect(aPaths).toContain('b-only.md');
    expect(a.readFile('b-only.md')).toBe('from B');
    expect(b.readFile('a-only.md')).toBe('from A');
  });

  it('write after initial sync still propagates', () => {
    const vid = '22222222-2222-4222-8222-222222222222';
    const a = new Doc(vid);
    const b = new Doc(vid);

    const aState = new SyncState();
    const bState = new SyncState();
    syncFully(a, aState, b, bState);

    a.writeTextFile('late.md', 'late');
    syncFully(a, aState, b, bState);

    expect(b.readFile('late.md')).toBe('late');
  });

  it('write after initial sync — single round (mirrors e2e flow)', () => {
    // The e2e Vault sends one sync message after a local write and waits
    // for the response. If a single round doesn't carry the change,
    // applications would have to keep poking the sync loop. This test
    // pins down the expected behavior: one outbound message must include
    // the new changes.
    const vid = '33333333-3333-4333-8333-333333333333';
    const a = new Doc(vid);
    const b = new Doc(vid);
    // Pre-seed b with a file (mirrors the hub already having a doc).
    b.writeTextFile('seed.md', 'seed');

    const aState = new SyncState();
    const bState = new SyncState();
    syncFully(a, aState, b, bState);
    expect(a.readFile('seed.md')).toBe('seed');

    // Local write on a, then ONE outbound message (no pull from b).
    a.writeTextFile('after.md', 'after');
    const m = a.generateSyncMessage(aState);
    expect(m).not.toBeUndefined();
    b.receiveSyncMessage(bState, m!);
    // b should now have the file — Automerge sync includes changes
    // in the outbound message when the sender knows the peer is missing them.
    expect(b.readFile('after.md')).toBe('after');
  });
});
