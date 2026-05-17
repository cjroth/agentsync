import { describe, expect, test } from 'bun:test';
import {
  type AgentsyncConfig,
  defaultConfig,
  parseConfig,
  serializeConfig,
} from '../../src/index.js';

// Byte-for-byte what `toml::to_string_pretty` emits for a typical vault —
// captured from a real `.agentsync/config.toml` the Rust CLI wrote.
const CLI_SAMPLE = `[vault]
id = "3ba23523-b267-447d-b842-e037fa12fed7"
name = "vault"
rendezvous_url = "wss://agentsync-production-ab4b.up.railway.app"
hub_pubkey = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIBAW38aulOaoGhJ81/wJBnCsdikGPuS4OKHD77aBnmBk"

[identity]

[sync]
extensions = [
    "md",
    "markdown",
]
include = []
attachment_max_bytes = 10485760
text_file_max_bytes = 1048576
log_retention_days = 30
`;

describe('parseConfig — CLI-written file', () => {
  test('projects onto the typed schema', () => {
    const { config } = parseConfig(CLI_SAMPLE);
    expect(config.vault.id).toBe('3ba23523-b267-447d-b842-e037fa12fed7');
    expect(config.vault.name).toBe('vault');
    expect(config.vault.rendezvous_url).toBe('wss://agentsync-production-ab4b.up.railway.app');
    expect(config.vault.hub_pubkey).toMatch(/^ssh-ed25519 /);
    expect(config.identity).toEqual({});
    expect(config.sync.extensions).toEqual(['md', 'markdown']);
    expect(config.sync.include).toEqual([]);
    expect(config.sync.attachment_max_bytes).toBe(10485760);
    expect(config.sync.text_file_max_bytes).toBe(1048576);
    expect(config.sync.log_retention_days).toBe(30);
  });

  test('round-trips byte-for-byte', () => {
    const { config, doc } = parseConfig(CLI_SAMPLE);
    expect(serializeConfig(config, doc)).toBe(CLI_SAMPLE);
  });
});

describe('lossless round-trip of unknown content', () => {
  test('unknown tables and keys survive a known-field edit', () => {
    const withExtras = `${CLI_SAMPLE}
[obsidian]
auto_connect = true

[future]
new_field = "keep me"
`;
    const { config, doc } = parseConfig(withExtras);
    config.vault.id = 'changed-id';
    const out = serializeConfig(config, doc);
    expect(out).toContain('id = "changed-id"');
    expect(out).toContain('[obsidian]');
    expect(out).toContain('auto_connect = true');
    expect(out).toContain('[future]');
    expect(out).toContain('new_field = "keep me"');
    // Re-parsing keeps the typed view consistent.
    expect(parseConfig(out).config.vault.id).toBe('changed-id');
  });
});

describe('serializeConfig — fresh', () => {
  test('emits canonical table order and fills sync defaults', () => {
    const cfg: AgentsyncConfig = defaultConfig();
    cfg.vault.id = 'v1';
    cfg.vault.rendezvous_url = 'wss://hub';
    const out = serializeConfig(cfg);
    expect(out).toBe(`[vault]
id = "v1"
rendezvous_url = "wss://hub"

[identity]

[sync]
extensions = [
    "md",
    "markdown",
]
include = []
attachment_max_bytes = 10485760
text_file_max_bytes = 1048576
log_retention_days = 30
`);
  });

  test('omits unset optional fields rather than writing empty strings', () => {
    const cfg = defaultConfig();
    cfg.vault.id = 'v1';
    const out = serializeConfig(cfg);
    expect(out).not.toContain('name =');
    expect(out).not.toContain('hub_pubkey =');
    expect(out).not.toContain('rendezvous_url =');
  });
});

describe('parser edge cases', () => {
  test('ignores comments (full-line and trailing) and blank lines', () => {
    const text = `# header comment

[vault]
id = "abc" # trailing comment
# another
name = "n"
`;
    const { config } = parseConfig(text);
    expect(config.vault.id).toBe('abc');
    expect(config.vault.name).toBe('n');
  });

  test('does not treat a # inside a quoted string as a comment', () => {
    const { config } = parseConfig('[vault]\nname = "a#b"\n');
    expect(config.vault.name).toBe('a#b');
  });

  test('handles inline and multiline string arrays', () => {
    const inline = parseConfig('[sync]\ninclude = ["**/*.md", "**/*.txt"]\n');
    expect(inline.config.sync.include).toEqual(['**/*.md', '**/*.txt']);
    const multi = parseConfig('[sync]\nextensions = [\n  "md",\n  "txt",\n]\n');
    expect(multi.config.sync.extensions).toEqual(['md', 'txt']);
  });

  test('string escapes round-trip', () => {
    const cfg = defaultConfig();
    cfg.vault.name = 'quote " and \\ and tab\t';
    const out = serializeConfig(cfg);
    expect(parseConfig(out).config.vault.name).toBe('quote " and \\ and tab\t');
  });

  test('booleans parse for unknown plugin keys', () => {
    const doc = parseConfig('[obsidian]\nauto_connect = false\n').doc;
    expect(doc.get('obsidian')?.get('auto_connect')).toBe(false);
  });
});
