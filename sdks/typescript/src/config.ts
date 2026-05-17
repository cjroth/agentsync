// `.agentsync/config.toml` model + a small, lossless TOML codec.
//
// This mirrors the Rust `ConfigFile` schema in `crates/agentsync-cli/src/
// config.rs` so the native CLI and any TS consumer (the Obsidian plugin)
// can share one `<vault>/.agentsync/config.toml` byte-for-byte. The codec
// only implements the TOML subset that schema needs — section tables,
// double-quoted strings, integers, booleans, and string arrays — but it
// round-trips *unknown* tables and keys untouched, so a newer CLI adding
// fields never has them clobbered by an older plugin (and vice versa).
//
// Serialization deliberately matches `toml::to_string_pretty`'s shape
// (multiline arrays, blank line between tables) so a file the CLI wrote
// and a file we wrote diff cleanly.

/** A scalar or string-array TOML value — the only shapes our schema uses. */
export type TomlValue = string | number | boolean | string[];

/**
 * An ordered TOML document: table name → (key → value), insertion order
 * preserved. The root (keys before any `[table]`) uses the `''` table key.
 */
export type TomlDoc = Map<string, Map<string, TomlValue>>;

// ---- Typed view of the agentsync schema ----

// Optional fields are `?: T | undefined` (not just `?: T`) so callers can
// clear them with an explicit `= undefined` under
// `exactOptionalPropertyTypes` without resorting to `delete`.
export interface VaultSection {
  id?: string | undefined;
  name?: string | undefined;
  rendezvous_url?: string | undefined;
  /** TOFU-pinned hub identity, SSH wire format (`ssh-ed25519 AAAA…`). */
  hub_pubkey?: string | undefined;
}

export interface IdentitySection {
  path?: string | undefined;
  agent_socket?: string | undefined;
  agent_pubkey?: string | undefined;
}

export interface SyncSection {
  extensions: string[];
  include: string[];
  attachment_max_bytes: number;
  text_file_max_bytes: number;
  log_retention_days: number;
}

export interface AgentsyncConfig {
  vault: VaultSection;
  identity: IdentitySection;
  sync: SyncSection;
}

/** Matches the `default_*` fns in `config.rs`. */
export function defaultSyncSection(): SyncSection {
  return {
    extensions: ['md', 'markdown'],
    include: [],
    attachment_max_bytes: 10 * 1024 * 1024,
    text_file_max_bytes: 1 * 1024 * 1024,
    log_retention_days: 30,
  };
}

export function defaultConfig(): AgentsyncConfig {
  return { vault: {}, identity: {}, sync: defaultSyncSection() };
}

// ---- Parser ----

/**
 * Parse TOML into an ordered {@link TomlDoc}. Tolerant of the subset our
 * schema uses; throws on input it can't represent rather than guessing.
 */
export function parseTomlDoc(text: string): TomlDoc {
  const doc: TomlDoc = new Map();
  let table = ensureTable(doc, '');
  const lines = text.split('\n');

  for (let i = 0; i < lines.length; i++) {
    const raw = lines[i] ?? '';
    const line = stripComment(raw).trim();
    if (line === '') continue;

    const header = /^\[([^\]]+)\]$/.exec(line);
    if (header) {
      table = ensureTable(doc, (header[1] ?? '').trim());
      continue;
    }

    const eq = line.indexOf('=');
    if (eq === -1) throw new Error(`config.toml: malformed line ${i + 1}: ${raw}`);
    const key = line.slice(0, eq).trim();
    let rhs = line.slice(eq + 1).trim();

    if (rhs.startsWith('[')) {
      // Array — may span multiple lines until the closing bracket.
      while (!hasClosingBracket(rhs)) {
        i += 1;
        if (i >= lines.length) throw new Error('config.toml: unterminated array');
        rhs += `\n${lines[i]}`;
      }
      table.set(key, parseStringArray(rhs));
    } else {
      table.set(key, parseScalar(rhs));
    }
  }
  return doc;
}

function ensureTable(doc: TomlDoc, name: string): Map<string, TomlValue> {
  let t = doc.get(name);
  if (!t) {
    t = new Map();
    doc.set(name, t);
  }
  return t;
}

/** Drop a trailing `# comment` that isn't inside a double-quoted string. */
function stripComment(line: string): string {
  let inStr = false;
  for (let i = 0; i < line.length; i++) {
    const c = line[i];
    if (c === '"' && line[i - 1] !== '\\') inStr = !inStr;
    else if (c === '#' && !inStr) return line.slice(0, i);
  }
  return line;
}

function hasClosingBracket(s: string): boolean {
  // Brackets never appear inside our values, so a literal scan is enough.
  return s.includes(']');
}

function parseScalar(s: string): TomlValue {
  if (s.startsWith('"')) return parseTomlString(s);
  if (s === 'true') return true;
  if (s === 'false') return false;
  if (/^[+-]?[0-9_]+$/.test(s)) {
    const n = Number(s.replace(/_/g, ''));
    if (!Number.isSafeInteger(n)) throw new Error(`config.toml: integer out of range: ${s}`);
    return n;
  }
  throw new Error(`config.toml: unsupported value: ${s}`);
}

function parseTomlString(s: string): string {
  if (!s.startsWith('"')) throw new Error(`config.toml: expected string, got: ${s}`);
  let out = '';
  for (let i = 1; i < s.length; i++) {
    const c = s.charAt(i);
    if (c === '"') return out;
    if (c !== '\\') {
      out += c;
      continue;
    }
    const e = s.charAt(i + 1);
    i += 1;
    if (e === 'n') out += '\n';
    else if (e === 't') out += '\t';
    else if (e === 'r') out += '\r';
    else if (e === '"') out += '"';
    else if (e === '\\') out += '\\';
    else if (e === 'u') {
      out += String.fromCharCode(Number.parseInt(s.slice(i + 1, i + 5), 16));
      i += 4;
    } else out += e;
  }
  throw new Error(`config.toml: unterminated string: ${s}`);
}

function parseStringArray(s: string): string[] {
  const inner = s.slice(s.indexOf('[') + 1, s.lastIndexOf(']'));
  const out: string[] = [];
  let i = 0;
  while (i < inner.length) {
    if (inner.charAt(i) !== '"') {
      i += 1; // whitespace, commas, newlines
      continue;
    }
    // Walk to the matching unescaped close quote, then reuse the string
    // parser so escapes are honored consistently.
    let j = i + 1;
    while (j < inner.length && !(inner.charAt(j) === '"' && inner.charAt(j - 1) !== '\\')) j++;
    out.push(parseTomlString(inner.slice(i, j + 1)));
    i = j + 1;
  }
  return out;
}

// ---- Serializer ----

function escapeTomlString(s: string): string {
  return s.replace(/\\/g, '\\\\').replace(/"/g, '\\"').replace(/\n/g, '\\n').replace(/\t/g, '\\t');
}

function formatValue(v: TomlValue): string {
  if (typeof v === 'string') return `"${escapeTomlString(v)}"`;
  if (typeof v === 'number' || typeof v === 'boolean') return String(v);
  if (v.length === 0) return '[]';
  return `[\n${v.map((e) => `    "${escapeTomlString(e)}",`).join('\n')}\n]`;
}

/** Render a {@link TomlDoc} the way `toml::to_string_pretty` would. */
export function stringifyTomlDoc(doc: TomlDoc): string {
  const blocks: string[] = [];
  const root = doc.get('');
  if (root && root.size > 0) {
    blocks.push([...root].map(([k, v]) => `${k} = ${formatValue(v)}`).join('\n'));
  }
  for (const [name, table] of doc) {
    if (name === '') continue;
    const body = [...table].map(([k, v]) => `${k} = ${formatValue(v)}`);
    blocks.push([`[${name}]`, ...body].join('\n'));
  }
  return blocks.length ? `${blocks.join('\n\n')}\n` : '';
}

// ---- Schema mapping (lossless: unknown tables/keys survive) ----

function strOf(t: Map<string, TomlValue> | undefined, k: string): string | undefined {
  const v = t?.get(k);
  return typeof v === 'string' ? v : undefined;
}
function strArr(v: TomlValue | undefined, fallback: string[]): string[] {
  return Array.isArray(v) ? v.slice() : fallback;
}
function intOf(v: TomlValue | undefined, fallback: number): number {
  return typeof v === 'number' ? v : fallback;
}

/** Build an object with only the defined entries — keeps optional fields
 * genuinely absent (required under `exactOptionalPropertyTypes`). */
function compact<T extends object>(entries: Record<string, string | undefined>): T {
  const out: Record<string, string> = {};
  for (const [k, v] of Object.entries(entries)) if (v !== undefined) out[k] = v;
  return out as T;
}

/** Project a parsed doc onto the typed agentsync schema (defaults filled). */
export function configFromDoc(doc: TomlDoc): AgentsyncConfig {
  const vault = doc.get('vault');
  const identity = doc.get('identity');
  const sync = doc.get('sync');
  const d = defaultSyncSection();
  return {
    vault: compact<VaultSection>({
      id: strOf(vault, 'id'),
      name: strOf(vault, 'name'),
      rendezvous_url: strOf(vault, 'rendezvous_url'),
      hub_pubkey: strOf(vault, 'hub_pubkey'),
    }),
    identity: compact<IdentitySection>({
      path: strOf(identity, 'path'),
      agent_socket: strOf(identity, 'agent_socket'),
      agent_pubkey: strOf(identity, 'agent_pubkey'),
    }),
    sync: {
      extensions: strArr(sync?.get('extensions'), d.extensions),
      include: strArr(sync?.get('include'), d.include),
      attachment_max_bytes: intOf(sync?.get('attachment_max_bytes'), d.attachment_max_bytes),
      text_file_max_bytes: intOf(sync?.get('text_file_max_bytes'), d.text_file_max_bytes),
      log_retention_days: intOf(sync?.get('log_retention_days'), d.log_retention_days),
    },
  };
}

/**
 * Write the typed schema back into `base` (a doc previously parsed from
 * disk, or empty), preserving any unknown tables/keys the CLI may have
 * written. Optional `vault`/`identity` fields are removed when unset so we
 * don't persist empty `key = ""` lines.
 */
export function applyConfigToDoc(cfg: AgentsyncConfig, base?: TomlDoc): TomlDoc {
  const doc: TomlDoc = base ?? new Map();
  const put = (table: string, key: string, val: string | undefined): void => {
    const t = ensureTable(doc, table);
    if (val === undefined || val === '') t.delete(key);
    else t.set(key, val);
  };
  // Keep canonical table order for freshly-created files.
  ensureTable(doc, 'vault');
  ensureTable(doc, 'identity');
  ensureTable(doc, 'sync');

  put('vault', 'id', cfg.vault.id);
  put('vault', 'name', cfg.vault.name);
  put('vault', 'rendezvous_url', cfg.vault.rendezvous_url);
  put('vault', 'hub_pubkey', cfg.vault.hub_pubkey);
  put('identity', 'path', cfg.identity.path);
  put('identity', 'agent_socket', cfg.identity.agent_socket);
  put('identity', 'agent_pubkey', cfg.identity.agent_pubkey);

  const sync = ensureTable(doc, 'sync');
  sync.set('extensions', cfg.sync.extensions.slice());
  sync.set('include', cfg.sync.include.slice());
  sync.set('attachment_max_bytes', cfg.sync.attachment_max_bytes);
  sync.set('text_file_max_bytes', cfg.sync.text_file_max_bytes);
  sync.set('log_retention_days', cfg.sync.log_retention_days);
  return doc;
}

/** Parse `config.toml` text into the typed schema + the raw doc (for
 * lossless re-serialization via {@link serializeConfig}). */
export function parseConfig(text: string): { config: AgentsyncConfig; doc: TomlDoc } {
  const doc = parseTomlDoc(text);
  return { config: configFromDoc(doc), doc };
}

/** Serialize the typed schema, layering it onto `baseDoc` if supplied so
 * unknown CLI-written content is preserved. */
export function serializeConfig(cfg: AgentsyncConfig, baseDoc?: TomlDoc): string {
  return stringifyTomlDoc(applyConfigToDoc(cfg, baseDoc));
}
