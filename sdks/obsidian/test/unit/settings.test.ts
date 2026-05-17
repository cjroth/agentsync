import { describe, expect, test } from 'bun:test';
import { parseTomlDoc, stringifyTomlDoc } from '@agentsync/sdk/web-init';
import {
  type AgentsyncSettings,
  ConfigStore,
  DEFAULT_SETTINGS,
  parseIgnoreGlobs,
  settingsFromTomlDoc,
  writeSettingsToTomlDoc,
} from '../../src/settings.js';
import { FakeDataAdapter } from '../mocks/obsidian.js';

const CLI_SAMPLE = `[vault]
id = "3ba23523-b267-447d-b842-e037fa12fed7"
name = "vault"
rendezvous_url = "wss://hub.example"
hub_pubkey = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIBexample"

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

describe('parseIgnoreGlobs', () => {
  test('splits lines, trims, drops empty + comment lines', () => {
    expect(parseIgnoreGlobs('  Drafts/**\n\n# comment\n*.tmp.md\n')).toEqual([
      'Drafts/**',
      '*.tmp.md',
    ]);
  });
  test('empty input → empty list', () => {
    expect(parseIgnoreGlobs('')).toEqual([]);
    expect(parseIgnoreGlobs('   \n  \n')).toEqual([]);
  });
});

describe('settingsFromTomlDoc', () => {
  test('maps a CLI-written config onto the settings view', () => {
    const s = settingsFromTomlDoc(parseTomlDoc(CLI_SAMPLE));
    expect(s.vaultId).toBe('3ba23523-b267-447d-b842-e037fa12fed7');
    expect(s.rendezvousUrl).toBe('wss://hub.example');
    expect(s.hubPubkey).toBe('ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIBexample');
    expect(s.autoConnectOnStart).toBe(false);
    expect(s.ignoreGlobs).toEqual([]);
  });

  test('empty doc → defaults', () => {
    expect(settingsFromTomlDoc(new Map())).toEqual(DEFAULT_SETTINGS);
  });

  test('reads plugin-only [obsidian] knobs', () => {
    const doc = parseTomlDoc(
      '[obsidian]\nsync_enabled = true\nauto_connect = true\nignore_globs = ["Drafts/**", "*.tmp.md"]\n',
    );
    const s = settingsFromTomlDoc(doc);
    expect(s.syncEnabled).toBe(true);
    expect(s.autoConnectOnStart).toBe(true);
    expect(s.ignoreGlobs).toEqual(['Drafts/**', '*.tmp.md']);
  });

  test('reads [vault] name and [identity] path', () => {
    const doc = parseTomlDoc(
      '[vault]\nname = "Notes"\n\n[identity]\npath = ".agentsync/id_ed25519"\n',
    );
    const s = settingsFromTomlDoc(doc);
    expect(s.vaultName).toBe('Notes');
    expect(s.identityPath).toBe('.agentsync/id_ed25519');
    expect(s.syncEnabled).toBe(false);
  });
});

describe('writeSettingsToTomlDoc', () => {
  test('round-trips through the settings view', () => {
    const base = parseTomlDoc(CLI_SAMPLE);
    const s = settingsFromTomlDoc(base);
    s.vaultId = 'new-id';
    s.autoConnectOnStart = true;
    s.ignoreGlobs = ['Z/**'];
    const out = stringifyTomlDoc(writeSettingsToTomlDoc(s, base));
    const back = settingsFromTomlDoc(parseTomlDoc(out));
    expect(back.vaultId).toBe('new-id');
    expect(back.autoConnectOnStart).toBe(true);
    expect(back.ignoreGlobs).toEqual(['Z/**']);
    // CLI-managed fields untouched.
    expect(out).toContain('name = "vault"');
    expect(out).toContain('attachment_max_bytes = 10485760');
  });

  test('drops empty vault keys and the [obsidian] table when unused', () => {
    const out = stringifyTomlDoc(writeSettingsToTomlDoc({ ...DEFAULT_SETTINGS }));
    expect(out).not.toContain('id =');
    expect(out).not.toContain('hub_pubkey =');
    expect(out).not.toContain('[obsidian]');
  });

  test('preserves unknown CLI tables/keys', () => {
    const base = parseTomlDoc(`${CLI_SAMPLE}\n[future]\nx = "keep"\n`);
    const s = settingsFromTomlDoc(base);
    s.rendezvousUrl = 'wss://changed';
    const out = stringifyTomlDoc(writeSettingsToTomlDoc(s, base));
    expect(out).toContain('rendezvous_url = "wss://changed"');
    expect(out).toContain('[future]');
    expect(out).toContain('x = "keep"');
  });
});

describe('ConfigStore', () => {
  test('load() returns defaults when config.toml is absent', async () => {
    const store = new ConfigStore(new FakeDataAdapter());
    expect(await store.load()).toEqual(DEFAULT_SETTINGS);
  });

  test('exists() reflects whether config.toml is present', async () => {
    const fs = new FakeDataAdapter();
    const store = new ConfigStore(fs);
    expect(await store.exists()).toBe(false);
    await store.save({ ...DEFAULT_SETTINGS, vaultId: 'v' });
    expect(await store.exists()).toBe(true);
  });

  test('save() then load() round-trips and writes to .agentsync/config.toml', async () => {
    const fs = new FakeDataAdapter();
    const store = new ConfigStore(fs);
    await store.load();
    const written: AgentsyncSettings = {
      rendezvousUrl: 'wss://hub',
      vaultId: 'v-1',
      vaultName: 'Notes',
      hubPubkey: 'ssh-ed25519 AAAApin',
      syncEnabled: true,
      autoConnectOnStart: true,
      onboarded: true,
      ignoreGlobs: ['Drafts/**'],
      identityPath: '.agentsync/id_ed25519',
    };
    await store.save(written);
    expect(await fs.exists('.agentsync/config.toml')).toBe(true);
    const text = await fs.read('.agentsync/config.toml');
    expect(text).toContain('[vault]');
    expect(text).toContain('id = "v-1"');
    expect(text).toContain('name = "Notes"');
    expect(text).toContain('path = ".agentsync/id_ed25519"');
    expect(text).toContain('sync_enabled = true');
    expect(text).toContain('onboarded = true');
    expect(text).toContain('[obsidian]');

    expect(await new ConfigStore(fs).load()).toEqual(written);
  });

  test('save() preserves a config.toml the CLI wrote', async () => {
    const fs = new FakeDataAdapter();
    await fs.mkdir('.agentsync');
    await fs.write('.agentsync/config.toml', CLI_SAMPLE);
    const store = new ConfigStore(fs);
    const s = await store.load();
    s.rendezvousUrl = 'wss://moved';
    await store.save(s);
    const text = await fs.read('.agentsync/config.toml');
    expect(text).toContain('rendezvous_url = "wss://moved"');
    expect(text).toContain('name = "vault"'); // CLI field survives
    expect(text).toContain('log_retention_days = 30');
  });
});
