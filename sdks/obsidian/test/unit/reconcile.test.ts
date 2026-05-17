import { describe, expect, test } from 'bun:test';
import { planReconcile } from '../../src/reconcile.js';

const ALL = (_p: string) => true;

describe('planReconcile', () => {
  test('empty inputs → empty plan', async () => {
    const p = await planReconcile({ obsidianFiles: [], sdkFiles: [], filter: ALL });
    expect(p.pushToSdk).toEqual([]);
    expect(p.applyToObsidian).toEqual([]);
    expect(p.deleteInObsidian).toEqual([]);
  });

  test('only-local → push to SDK', async () => {
    const p = await planReconcile({
      obsidianFiles: [{ path: 'a.md', readText: async () => 'hello' }],
      sdkFiles: [],
      filter: ALL,
    });
    expect(p.pushToSdk).toEqual([{ path: 'a.md', content: 'hello' }]);
    expect(p.applyToObsidian).toEqual([]);
    expect(p.deleteInObsidian).toEqual([]);
  });

  test('only-remote alive → apply to Obsidian', async () => {
    const p = await planReconcile({
      obsidianFiles: [],
      sdkFiles: [{ path: 'b.md', deleted: false, readText: async () => 'world' }],
      filter: ALL,
    });
    expect(p.pushToSdk).toEqual([]);
    expect(p.applyToObsidian).toEqual([{ path: 'b.md', content: 'world', create: true }]);
  });

  test('only-remote tombstoned → no-op (not present locally)', async () => {
    const p = await planReconcile({
      obsidianFiles: [],
      sdkFiles: [{ path: 'gone.md', deleted: true, readText: async () => 'x' }],
      filter: ALL,
    });
    expect(p.pushToSdk).toEqual([]);
    expect(p.applyToObsidian).toEqual([]);
    expect(p.deleteInObsidian).toEqual([]);
  });

  test('both alive equal → no-op', async () => {
    const p = await planReconcile({
      obsidianFiles: [{ path: 'c.md', readText: async () => 'same' }],
      sdkFiles: [{ path: 'c.md', deleted: false, readText: async () => 'same' }],
      filter: ALL,
    });
    expect(p.pushToSdk).toEqual([]);
    expect(p.applyToObsidian).toEqual([]);
  });

  test('both alive differ → push obsidian to SDK (Automerge merges)', async () => {
    const p = await planReconcile({
      obsidianFiles: [{ path: 'd.md', readText: async () => 'local' }],
      sdkFiles: [{ path: 'd.md', deleted: false, readText: async () => 'remote' }],
      filter: ALL,
    });
    expect(p.pushToSdk).toEqual([{ path: 'd.md', content: 'local' }]);
    expect(p.applyToObsidian).toEqual([]);
  });

  test('tombstoned remote + alive local → delete in Obsidian', async () => {
    const p = await planReconcile({
      obsidianFiles: [{ path: 'e.md', readText: async () => 'kept' }],
      sdkFiles: [{ path: 'e.md', deleted: true, readText: async () => '' }],
      filter: ALL,
    });
    expect(p.pushToSdk).toEqual([]);
    expect(p.deleteInObsidian).toEqual(['e.md']);
  });

  test('filter excludes paths from both sides', async () => {
    const filter = (p: string) => p.endsWith('.md');
    const plan = await planReconcile({
      obsidianFiles: [
        { path: 'a.md', readText: async () => 'x' },
        { path: 'img.png', readText: async () => 'binary' },
      ],
      sdkFiles: [{ path: 'b.png', deleted: false, readText: async () => 'b' }],
      filter,
    });
    expect(plan.pushToSdk).toEqual([{ path: 'a.md', content: 'x' }]);
    expect(plan.applyToObsidian).toEqual([]);
  });
});
