import { describe, expect, test } from 'bun:test';
import { Doc, contentHash, schemaVersion } from '../../src/index.js';

describe('Doc', () => {
  test('new + write + read round-trips', () => {
    const doc = new Doc('vault-1');
    expect(doc.vaultId()).toBe('vault-1');
    doc.writeTextFile('notes/hello.md', '# hello\n');
    expect(doc.readFile('notes/hello.md')).toBe('# hello\n');
    expect(doc.fileExists('notes/hello.md')).toBe(true);
    expect(doc.fileExists('notes/missing.md')).toBe(false);
  });

  test('save + load preserves content', () => {
    const doc = new Doc('vault-2');
    doc.writeTextFile('a.md', 'A');
    doc.writeTextFile('b.md', 'B');
    const bytes = doc.save();
    const loaded = Doc.load(bytes);
    expect(loaded.vaultId()).toBe('vault-2');
    expect(loaded.readFile('a.md')).toBe('A');
    expect(loaded.readFile('b.md')).toBe('B');
  });

  test('two peers merge concurrent edits without conflict', () => {
    const a = new Doc('shared');
    a.writeTextFile('seed.md', 'seed');
    const b = Doc.load(a.save());
    a.writeTextFile('a-only.md', 'from a');
    b.writeTextFile('b-only.md', 'from b');
    const bClone = Doc.load(b.save());
    expect(a.merge(bClone)).toBe(true);
    expect(a.readFile('seed.md')).toBe('seed');
    expect(a.readFile('a-only.md')).toBe('from a');
    expect(a.readFile('b-only.md')).toBe('from b');
  });

  test('deleteFile removes the file', () => {
    const doc = new Doc('v');
    doc.writeTextFile('x.md', 'x');
    expect(doc.fileExists('x.md')).toBe(true);
    doc.deleteFile('x.md');
    expect(doc.fileExists('x.md')).toBe(false);
  });

  test('schemaVersion is stable', () => {
    expect(schemaVersion()).toBe(1);
  });

  test('contentHash matches sha256 hex', () => {
    expect(contentHash(new Uint8Array([0x68, 0x69]))).toBe(
      '8f434346648f6b96df89dda901c5176b10a6d83961dd3c1ac88b59b2dc327aa4',
    );
  });
});
