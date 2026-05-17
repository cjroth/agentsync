import { describe, expect, test } from 'bun:test';
import {
  TEXT_EXTS,
  extOf,
  globToRegex,
  isTextPath,
  matchesAnyGlob,
  shouldSync,
} from '../../src/path-filter.js';

describe('extOf', () => {
  test('returns lowercase extension', () => {
    expect(extOf('foo.MD')).toBe('md');
    expect(extOf('foo.png')).toBe('png');
  });
  test('returns empty string for no extension', () => {
    expect(extOf('foo')).toBe('');
    expect(extOf('foo/bar')).toBe('');
  });
  test('returns empty string for dotfiles', () => {
    expect(extOf('.gitignore')).toBe('');
    expect(extOf('foo/.env')).toBe('');
  });
  test('handles deep paths', () => {
    expect(extOf('a/b/c.md')).toBe('md');
  });
  test('uses last dot only', () => {
    expect(extOf('archive.tar.gz')).toBe('gz');
  });
});

describe('isTextPath', () => {
  test('returns true for every TEXT_EXTS entry', () => {
    for (const ext of TEXT_EXTS) {
      expect(isTextPath(`note.${ext}`)).toBe(true);
    }
  });
  test('returns false for binary extensions', () => {
    for (const ext of ['png', 'jpg', 'pdf', 'mp3', 'excalidraw', 'gif', 'mp4']) {
      expect(isTextPath(`x.${ext}`)).toBe(false);
    }
  });
  test('returns false for empty path', () => {
    expect(isTextPath('')).toBe(false);
  });
  test('returns false for extension-less files', () => {
    expect(isTextPath('LICENSE')).toBe(false);
  });
});

describe('globToRegex', () => {
  test('plain literal', () => {
    expect(globToRegex('hello.md').test('hello.md')).toBe(true);
    expect(globToRegex('hello.md').test('Xhello.md')).toBe(false);
  });
  test('single-* matches within a segment', () => {
    expect(globToRegex('Drafts/*.md').test('Drafts/foo.md')).toBe(true);
    expect(globToRegex('Drafts/*.md').test('Drafts/sub/foo.md')).toBe(false);
  });
  test('double-** crosses segments', () => {
    expect(globToRegex('Drafts/**').test('Drafts/sub/foo.md')).toBe(true);
    expect(globToRegex('**/foo.md').test('a/b/foo.md')).toBe(true);
  });
  test('? matches a single character', () => {
    expect(globToRegex('?.md').test('a.md')).toBe(true);
    expect(globToRegex('?.md').test('ab.md')).toBe(false);
  });
  test('escapes regex metacharacters', () => {
    expect(globToRegex('foo.bar+baz.md').test('foo.bar+baz.md')).toBe(true);
    expect(globToRegex('foo.bar+baz.md').test('fooXbarYbaz.md')).toBe(false);
  });
});

describe('matchesAnyGlob', () => {
  test('false on empty list', () => {
    expect(matchesAnyGlob('foo.md', [])).toBe(false);
  });
  test('true if any glob matches', () => {
    expect(matchesAnyGlob('Drafts/x.md', ['*.tmp', 'Drafts/**'])).toBe(true);
  });
  test('false if no glob matches', () => {
    expect(matchesAnyGlob('Notes/x.md', ['Drafts/**'])).toBe(false);
  });
  test('skips empty/whitespace globs', () => {
    expect(matchesAnyGlob('foo.md', ['', 'foo.md'])).toBe(true);
    expect(matchesAnyGlob('foo.md', [''])).toBe(false);
  });
});

describe('shouldSync', () => {
  test('rejects empty path', () => {
    expect(shouldSync('', [])).toBe(false);
  });
  test('rejects SDK-reserved authorized_keys', () => {
    expect(shouldSync('authorized_keys', [])).toBe(false);
  });
  test('rejects binary files even without ignore globs', () => {
    expect(shouldSync('img.png', [])).toBe(false);
  });
  test('rejects when ignore glob matches', () => {
    expect(shouldSync('Drafts/foo.md', ['Drafts/**'])).toBe(false);
  });
  test('accepts text files when nothing matches', () => {
    expect(shouldSync('Notes/foo.md', ['Drafts/**'])).toBe(true);
    expect(shouldSync('foo.canvas', [])).toBe(true);
  });
});
