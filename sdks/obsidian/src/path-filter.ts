// Decides which vault paths should sync. Pure functions, no Obsidian
// runtime dependencies — fully unit-testable.
//
// Rules:
//   1. Only files with one of TEXT_EXTS sync. Binary attachments (images,
//      PDFs, .excalidraw, …) are skipped in v1 — the SDK's blob CAS isn't
//      wired up yet.
//   2. The user's ignore globs are applied on top. A glob match → skip.
//   3. Empty paths and the special `authorized_keys` file (managed by the
//      SDK) are always skipped.
//
// Glob syntax we support (intentionally minimal — no external dep):
//   `*`   — match any sequence of non-`/` characters
//   `**`  — match any sequence including `/`
//   `?`   — match a single non-`/` character
//   anything else is a literal

/** File extensions we consider "text" — sync these. Lowercase, no dot. */
export const TEXT_EXTS: ReadonlySet<string> = new Set([
  'md',
  'mdx',
  'txt',
  'canvas',
  'json',
  'css',
  'yaml',
  'yml',
  'csv',
]);

/** Special path inside the SDK doc that the plugin must never push back. */
const SDK_RESERVED_PATHS: ReadonlySet<string> = new Set(['authorized_keys']);

/** Return the lowercase extension (no dot) of `path`, or '' if none. */
export function extOf(path: string): string {
  const slash = path.lastIndexOf('/');
  const base = slash === -1 ? path : path.slice(slash + 1);
  const dot = base.lastIndexOf('.');
  if (dot <= 0) return '';
  return base.slice(dot + 1).toLowerCase();
}

/** True if `path` ends in a text extension we sync. */
export function isTextPath(path: string): boolean {
  if (!path) return false;
  return TEXT_EXTS.has(extOf(path));
}

/**
 * Compile a glob to a regex. Exposed for testing; callers should use
 * `matchesAnyGlob` or `shouldSync`.
 */
export function globToRegex(glob: string): RegExp {
  let re = '^';
  for (let i = 0; i < glob.length; i++) {
    const c = glob[i] as string;
    if (c === '*') {
      if (glob[i + 1] === '*') {
        re += '.*';
        i++;
      } else {
        re += '[^/]*';
      }
    } else if (c === '?') {
      re += '[^/]';
    } else if ('\\^$+.()|{}[]'.includes(c)) {
      re += `\\${c}`;
    } else {
      re += c;
    }
  }
  re += '$';
  return new RegExp(re);
}

/** True if `path` matches any of `globs`. Empty list returns false. */
export function matchesAnyGlob(path: string, globs: readonly string[]): boolean {
  for (const g of globs) {
    if (!g) continue;
    if (globToRegex(g).test(path)) return true;
  }
  return false;
}

/**
 * Top-level decision: should the plugin sync this path? Combines the
 * text-extension allowlist, SDK-reserved paths, and user ignore globs.
 */
export function shouldSync(path: string, ignoreGlobs: readonly string[]): boolean {
  if (!path) return false;
  if (SDK_RESERVED_PATHS.has(path)) return false;
  if (!isTextPath(path)) return false;
  if (matchesAnyGlob(path, ignoreGlobs)) return false;
  return true;
}
