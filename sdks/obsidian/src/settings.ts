// The plugin's settings ARE `<vault>/.agentsync/config.toml` — the exact
// same file the native `agentsync` CLI reads and writes. There is no
// separate `data.json`: a vault directory works identically whether you
// drive it from the CLI or this plugin, which structurally removes the
// "configured vault id vs. on-disk doc" divergence class of bugs.
//
// Schema-defined fields (`[vault]`, `[identity]`, `[sync]`) are shared
// with the CLI. Plugin-only knobs the CLI has no concept of live in an
// `[obsidian]` table; the Rust `ConfigFile` has no `deny_unknown_fields`,
// so the CLI silently ignores that table and we preserve everything it
// writes on round-trip.
//
// Pure module — no `obsidian` runtime import — so it is fully unit
// testable. File IO is injected via `MinimalDataAdapter`.

import {
  type AgentsyncConfig,
  type TomlDoc,
  type TomlValue,
  applyConfigToDoc,
  configFromDoc,
  parseTomlDoc,
  stringifyTomlDoc,
} from '@agentsync/sdk/web-init';
import type { MinimalDataAdapter } from './storage-adapter.js';

/** Plugin-only TOML table the CLI ignores. */
const OBSIDIAN_TABLE = 'obsidian';

export interface AgentsyncSettings {
  /** `[vault] rendezvous_url` — hub WebSocket URL. */
  rendezvousUrl: string;
  /** `[vault] id` — UUID of the remote vault. Empty → minted on first connect. */
  vaultId: string;
  /** `[vault] name` — human-readable label (CLI-shared). Optional. */
  vaultName: string;
  /** `[vault] hub_pubkey` — TOFU-pinned hub identity, SSH wire format
   * (`ssh-ed25519 AAAA…`). Empty until first successful connect. */
  hubPubkey: string;
  /** `[obsidian] sync_enabled` — master switch. The plugin builds no SDK
   * Vault and makes no connection while this is false. Set true by the
   * setup flow; toggled in settings. */
  syncEnabled: boolean;
  /** `[obsidian] auto_connect` — open the hub connection on launch (vs.
   * staying prepared). Only meaningful while `syncEnabled`. */
  autoConnectOnStart: boolean;
  /** `[obsidian] onboarded` — true only once setup fully succeeded
   * (create: local vault built; connect: hub handshake reached). Until
   * then the plugin keeps showing the setup wizard even though
   * `config.toml` already exists. The CLI ignores this key. */
  onboarded: boolean;
  /** `[obsidian] ignore_globs` — extra globs to skip, on top of the
   * always-on binary-extension filter. */
  ignoreGlobs: string[];
  /** `[identity] path` — vault-relative identity location. Set on mobile
   * (no home dir); unset on desktop so it defaults to the CLI-shared
   * `~/.agentsync/id_ed25519`. */
  identityPath: string;
}

export const DEFAULT_SETTINGS: AgentsyncSettings = {
  rendezvousUrl: '',
  vaultId: '',
  vaultName: '',
  hubPubkey: '',
  syncEnabled: false,
  // Default OFF: a first connect against a populated remote can pull
  // hundreds of files; the user opts in after configuring URL + id.
  autoConnectOnStart: false,
  onboarded: false,
  ignoreGlobs: [],
  identityPath: '',
};

/** Parse a textarea value (one glob per line) into a clean list. */
export function parseIgnoreGlobs(input: string): string[] {
  return input
    .split('\n')
    .map((s) => s.trim())
    .filter((s) => s.length > 0 && !s.startsWith('#'));
}

// ---- Pure config.toml ⇄ settings mapping (unit-testable) ----

function obsidianTable(doc: TomlDoc): Map<string, TomlValue> | undefined {
  return doc.get(OBSIDIAN_TABLE);
}

/** Project a parsed `config.toml` doc onto the plugin's settings view. */
export function settingsFromTomlDoc(doc: TomlDoc): AgentsyncSettings {
  const cfg = configFromDoc(doc);
  const ob = obsidianTable(doc);
  const auto = ob?.get('auto_connect');
  const sync = ob?.get('sync_enabled');
  const onboarded = ob?.get('onboarded');
  const globs = ob?.get('ignore_globs');
  return {
    rendezvousUrl: cfg.vault.rendezvous_url ?? '',
    vaultId: cfg.vault.id ?? '',
    vaultName: cfg.vault.name ?? '',
    hubPubkey: cfg.vault.hub_pubkey ?? '',
    syncEnabled: sync === true,
    autoConnectOnStart: auto === true,
    onboarded: onboarded === true,
    ignoreGlobs: Array.isArray(globs) ? globs.slice() : [],
    identityPath: cfg.identity.path ?? '',
  };
}

/**
 * Layer settings back onto `base` (the doc parsed from disk, so unknown
 * CLI-written content survives). Empty/false plugin knobs are removed so a
 * default config is byte-identical to what the CLI would write — no stray
 * empty `[obsidian]` table.
 */
export function writeSettingsToTomlDoc(s: AgentsyncSettings, base?: TomlDoc): TomlDoc {
  const prev: AgentsyncConfig = configFromDoc(base ?? new Map());
  // Only the three vault fields the plugin owns are overwritten; name,
  // identity.*, and sync.* are preserved as loaded. Empty → drop the key
  // (so we never persist `id = ""`).
  // Empty string → undefined; applyConfigToDoc drops undefined/'' keys so
  // we never persist `id = ""`.
  prev.vault.id = s.vaultId || undefined;
  prev.vault.name = s.vaultName || undefined;
  prev.vault.rendezvous_url = s.rendezvousUrl || undefined;
  prev.vault.hub_pubkey = s.hubPubkey || undefined;
  prev.identity.path = s.identityPath || undefined;
  const doc = applyConfigToDoc(prev, base);

  const ob = new Map<string, TomlValue>();
  if (s.syncEnabled) ob.set('sync_enabled', true);
  if (s.autoConnectOnStart) ob.set('auto_connect', true);
  if (s.onboarded) ob.set('onboarded', true);
  if (s.ignoreGlobs.length > 0) ob.set('ignore_globs', s.ignoreGlobs.slice());
  if (ob.size > 0) doc.set(OBSIDIAN_TABLE, ob);
  else doc.delete(OBSIDIAN_TABLE);
  return doc;
}

// ---- File-backed store ----

/**
 * Reads/writes `<vault-root>/.agentsync/config.toml`. The last-parsed doc
 * is retained so saves are lossless w.r.t. anything the CLI wrote that the
 * plugin doesn't model.
 */
export class ConfigStore {
  static readonly PATH = '.agentsync/config.toml';
  private doc: TomlDoc = new Map();
  /** Serializes saves: connect-mode setup can fire two near-simultaneous
   * writes (the TOFU hub-pin on `connected` and the onboarding latch),
   * and the tmp-write/rename dance is not concurrency-safe. */
  private writeChain: Promise<void> = Promise.resolve();

  constructor(private readonly adapter: MinimalDataAdapter) {}

  /** True once setup has written `config.toml`. The plugin treats this as
   * the single "is this vault configured?" signal — nothing touches
   * `.agentsync/` until it is. */
  exists(): Promise<boolean> {
    return this.adapter.exists(ConfigStore.PATH);
  }

  async load(): Promise<AgentsyncSettings> {
    if (await this.adapter.exists(ConfigStore.PATH)) {
      const text = await this.adapter.read(ConfigStore.PATH);
      this.doc = parseTomlDoc(text);
    } else {
      this.doc = new Map();
    }
    return settingsFromTomlDoc(this.doc);
  }

  save(s: AgentsyncSettings): Promise<void> {
    // Chain off the previous write (success or failure) so writes never
    // interleave their tmp/rename steps.
    const next = this.writeChain.then(
      () => this.doSave(s),
      () => this.doSave(s),
    );
    this.writeChain = next.then(
      () => {},
      () => {},
    );
    return next;
  }

  private async doSave(s: AgentsyncSettings): Promise<void> {
    this.doc = writeSettingsToTomlDoc(s, this.doc);
    const text = stringifyTomlDoc(this.doc);
    if (!(await this.adapter.exists('.agentsync'))) {
      await this.adapter.mkdir('.agentsync');
    }
    const tmp = `${ConfigStore.PATH}.tmp`;
    await this.adapter.write(tmp, text);
    if (await this.adapter.exists(ConfigStore.PATH)) {
      await this.adapter.remove(ConfigStore.PATH);
    }
    await this.adapter.rename(tmp, ConfigStore.PATH);
  }
}
