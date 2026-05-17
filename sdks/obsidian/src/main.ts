// Plugin entry point. Owns the lifecycle of the SyncController, registers
// commands, the settings tab, the status bar, and the Obsidian event
// listeners that drive Obsidian → SDK pushes.
//
// Lifecycle:
//   unconfigured ──runSetup()──┬─(succeeds)─> configured ──sync on/off──
//                              └─(fails)────> stays unconfigured; the
//                                  wizard stays up, pre-filled, with the
//                                  error so the user can fix and retry.
//
// "Configured" is gated on `[obsidian] onboarded`, NOT merely on
// `config.toml` existing. Setup writes config.toml up front (it's the
// CLI-shared file — a partial config is harmless), but the vault only
// counts as configured once setup actually works end to end: create-mode
// latches as soon as the local vault is built; connect-mode only after
// the hub handshake succeeds (the exact step where a wrong vault id or
// unauthorized device key fails). On load we build/start the controller
// only when `onboarded`; deleting `.agentsync/` returns the plugin to
// the unconfigured state rather than silently regenerating anything.
//
// The wasm bytes are inlined at build time by esbuild via the
// `__AGENTSYNC_WASM_B64__` define — no fetch at runtime, which is the only
// way mobile WebViews reliably load WebAssembly.

import { type IdentityInstance, initAgentsync, isInitialized } from '@agentsync/sdk/web-init';
import { Notice, Platform, Plugin, type TAbstractFile, type TFile } from 'obsidian';
import {
  type IdentityIO,
  NodeHomeIdentityIO,
  VAULT_IDENTITY_PATH,
  VaultAdapterIdentityIO,
  loadIdentity,
  loadOrCreateIdentity,
} from './identity-store.js';
import { AgentsyncSettingTab } from './settings-tab.js';
import { type AgentsyncSettings, ConfigStore, DEFAULT_SETTINGS } from './settings.js';
import { StatusBar } from './status-bar.js';
import { ObsidianStorageAdapter } from './storage-adapter.js';
import { SyncController } from './sync-controller.js';

declare const __AGENTSYNC_WASM_B64__: string;

/** Decode the build-time inlined wasm base64 into a Uint8Array. */
export function decodeInlinedWasm(b64: string): Uint8Array {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

export interface SetupOptions {
  mode: 'create' | 'connect';
  /** Create-mode: optional human-readable vault name. */
  vaultName?: string;
  /** Connect-mode: the existing remote vault id (required). */
  vaultId?: string;
  /** Hub WebSocket URL (required for connect, optional for create). */
  rendezvousUrl?: string;
}

export default class AgentsyncPlugin extends Plugin {
  settings: AgentsyncSettings = { ...DEFAULT_SETTINGS };
  controller: SyncController | null = null;
  statusBar: StatusBar | null = null;
  /** True once setup completed end to end (`[obsidian] onboarded`) —
   * drives the settings-tab UI (wizard vs. configured view). */
  configured = false;
  /** Last setup failure, surfaced in the wizard. Cleared on success. */
  onboardingError: string | null = null;

  private configStore: ConfigStore | null = null;
  private storage: ObsidianStorageAdapter | null = null;
  private wasmBytes: Uint8Array | null = null;
  private identity: IdentityInstance | null = null;
  /** Most recent controller notice, so a connect failure during setup
   * can be reported with the real cause rather than a generic message. */
  private lastNotice: string | null = null;

  override async onload(): Promise<void> {
    this.configStore = new ConfigStore(this.app.vault.adapter);
    this.storage = new ObsidianStorageAdapter(this.app.vault.adapter);
    this.wasmBytes = decodeInlinedWasm(__AGENTSYNC_WASM_B64__);

    const statusEl = this.addStatusBarItem();
    this.statusBar = new StatusBar(statusEl);
    this.statusBar.onClick(() => {
      // biome-ignore lint/suspicious/noExplicitAny: app.setting is private API.
      (this.app as any).setting?.open?.();
      // biome-ignore lint/suspicious/noExplicitAny: ditto.
      (this.app as any).setting?.openTabById?.(this.manifest.id);
    });

    this.addSettingTab(new AgentsyncSettingTab(this.app, this));
    this.registerObsidianEventListeners();
    this.registerCommands();

    if (await this.configStore.exists()) {
      // Load even when not yet onboarded so the wizard can pre-fill from
      // a previous incomplete attempt.
      this.settings = await this.configStore.load();
    }
    this.configured = this.settings.onboarded === true;
    if (!this.configured) {
      // Either never set up, or a setup that wrote config.toml but never
      // completed — show the wizard rather than auto-starting, which
      // would just re-fail the same way.
      this.statusBar.set('idle');
      return;
    }

    // Defer the controller start — and the vault reconcile it triggers —
    // until Obsidian has finished restoring its workspace. Before that
    // the metadata cache is cold and getAbstractFileByPath reports files
    // that physically exist (a prior session's sync) as absent, which
    // made the reconcile try to re-create them ("File/Folder already
    // exists") and abort the connect. onLayoutReady fires immediately if
    // the layout is already ready (e.g. enabling the plugin by hand).
    this.app.workspace.onLayoutReady(() => {
      void (async () => {
        try {
          await this.ensureController();
          if (this.settings.syncEnabled) {
            const connect = this.settings.autoConnectOnStart && !!this.settings.rendezvousUrl;
            await this.controller?.start({ connect });
          }
        } catch (err) {
          console.error('[agentsync] start failed:', err);
          new Notice(`Agentsync: ${err}`);
        }
      })();
    });
  }

  override async onunload(): Promise<void> {
    await this.controller?.stop();
    this.controller = null;
    this.identity?.free();
    this.identity = null;
  }

  // ---- Setup / lifecycle API (used by the settings tab) ----

  isConfigured(): boolean {
    return this.configured;
  }

  /** The one and only path that creates `<vault>/.agentsync/`. Resolves
   * (or generates) the device identity, writes `config.toml`, then brings
   * the controller online. */
  async runSetup(opts: SetupOptions): Promise<void> {
    await this.initWasm();
    const io = this.resolveIdentityIO();
    const { identity } = await loadOrCreateIdentity(io);
    this.identity?.free();
    this.identity = identity;

    const s: AgentsyncSettings = { ...DEFAULT_SETTINGS };
    s.syncEnabled = true;
    if (opts.rendezvousUrl) s.rendezvousUrl = opts.rendezvousUrl.trim();
    if (opts.mode === 'connect') {
      s.vaultId = (opts.vaultId ?? '').trim();
    } else if (opts.vaultName) {
      s.vaultName = opts.vaultName.trim();
    }
    // Mobile keeps the key in-vault; record it so a CLI on a synced copy
    // resolves the same file. Desktop leaves it unset (CLI default).
    if (!Platform.isDesktopApp) s.identityPath = VAULT_IDENTITY_PATH;

    // Persist config.toml up front (CLI-shared; a partial config is
    // harmless) but DON'T latch "configured" yet — `onboarded` flips
    // only once setup actually works. Until then the settings tab keeps
    // showing the wizard, pre-filled from these values.
    s.onboarded = false;
    this.settings = s;
    await this.configStore?.save(s); // ← creates .agentsync/config.toml
    this.configured = false;
    this.onboardingError = null;

    this.buildController();
    try {
      // create-mode mints the id locally; connect-mode joins the
      // configured one.
      await this.controller?.start({ connect: !!s.rendezvousUrl });
      // Connect-mode isn't done until the hub handshake succeeds — that
      // is where a wrong vault id / unauthorized key fails. Create-mode
      // is done as soon as the local vault exists.
      if (opts.mode === 'connect') await this.waitForConnect();
      await this.completeOnboarding();
    } catch (err) {
      this.onboardingError = String(err);
      // Tear the failed attempt down so a stale reconnect loop doesn't
      // keep erroring behind the wizard. config.toml is kept on purpose;
      // the user fixes the value and retries.
      await this.controller?.stop().catch(() => {});
      this.controller = null;
      throw err;
    }
  }

  /** Latch the vault as fully configured: persist `onboarded`, flip the
   * UI gate, clear any prior error. Idempotent. */
  private async completeOnboarding(): Promise<void> {
    this.settings.onboarded = true;
    await this.configStore?.save(this.settings);
    this.configured = true;
    this.onboardingError = null;
  }

  /** Resolve once the controller reaches `connected`; reject on `error`
   * or after `timeoutMs`. Makes connect-mode setup synchronous so the
   * wizard only flips to the configured view on a real connection. */
  private waitForConnect(timeoutMs = 30_000): Promise<void> {
    const c = this.controller;
    if (!c) return Promise.reject(new Error('controller not built'));
    if (c.state === 'connected') return Promise.resolve();
    return new Promise<void>((resolve, reject) => {
      const timer = setTimeout(() => {
        off();
        reject(new Error('timed out waiting for the hub connection'));
      }, timeoutMs);
      const off = c.on((st) => {
        if (st === 'connected') {
          clearTimeout(timer);
          off();
          resolve();
        } else if (st === 'error') {
          clearTimeout(timer);
          off();
          reject(new Error(this.lastNotice ?? 'connection failed'));
        }
      });
    });
  }

  /** Flip the master sync switch. Persists to `[obsidian] sync_enabled`
   * and starts/stops the controller accordingly. */
  async setSyncEnabled(on: boolean): Promise<void> {
    if (!this.configured) return;
    this.settings.syncEnabled = on;
    await this.configStore?.save(this.settings);
    if (on) {
      await this.ensureController();
      const connect = this.settings.autoConnectOnStart && !!this.settings.rendezvousUrl;
      await this.controller?.start({ connect });
    } else {
      await this.controller?.stop();
    }
  }

  async saveSettings(): Promise<void> {
    await this.configStore?.save(this.settings);
  }

  // ---- Internal ----

  private async initWasm(): Promise<void> {
    if (!isInitialized() && this.wasmBytes) await initAgentsync(this.wasmBytes);
  }

  private resolveIdentityIO(): IdentityIO {
    return Platform.isDesktopApp
      ? new NodeHomeIdentityIO()
      : new VaultAdapterIdentityIO(this.app.vault.adapter);
  }

  /** Load the (already-created) identity and construct the controller.
   * Throws if configured but the identity is missing — we never silently
   * regenerate a key; the user must re-run setup. */
  private async ensureController(): Promise<void> {
    if (this.controller) return;
    await this.initWasm();
    if (!this.identity) {
      const io = this.resolveIdentityIO();
      const identity = await loadIdentity(io);
      if (!identity) {
        throw new Error(
          `device identity not found at ${io.describe()} — run Agentsync setup again to generate one`,
        );
      }
      this.identity = identity;
    }
    this.buildController();
  }

  private buildController(): void {
    if (this.controller || !this.storage || !this.identity || !this.wasmBytes) return;
    this.controller = new SyncController({
      storage: this.storage,
      vault: this.app.vault,
      settings: this.settings,
      identity: this.identity,
      saveSettings: async (s) => {
        this.settings = s;
        await this.configStore?.save(s);
      },
      wasmBytes: this.wasmBytes,
      notice: (m) => {
        this.lastNotice = m;
        new Notice(m);
      },
      log: (m) => console.log('[agentsync]', m),
    });
    this.controller.on((st) => this.statusBar?.set(st));
  }

  private registerCommands(): void {
    this.addCommand({
      id: 'agentsync-connect',
      name: 'Connect to hub',
      callback: () => {
        if (!this.configured) {
          new Notice('Agentsync: not set up yet — open settings to configure.');
          return;
        }
        void this.controller?.start();
      },
    });
    this.addCommand({
      id: 'agentsync-disconnect',
      name: 'Disconnect from hub',
      callback: () => {
        void this.controller?.stop();
      },
    });
    this.addCommand({
      id: 'agentsync-resync',
      name: 'Resync now',
      callback: () => {
        void this.controller?.resyncNow();
      },
    });
    this.addCommand({
      id: 'agentsync-copy-pubkey',
      name: 'Copy device public key',
      callback: async () => {
        const ssh = this.controller?.identityPubkeySsh();
        if (!ssh) {
          new Notice('Agentsync: not set up yet.');
          return;
        }
        await navigator.clipboard.writeText(ssh);
        new Notice('Public key copied to clipboard.');
      },
    });
    this.addCommand({
      id: 'agentsync-create-snapshot',
      name: 'Create snapshot label',
      callback: async () => {
        if (!this.controller) {
          new Notice('Agentsync: not set up yet.');
          return;
        }
        const name = `snapshot-${new Date().toISOString().replace(/[:.]/g, '-')}`;
        await this.controller.createLabel(name);
        new Notice(`Snapshot created: ${name}`);
      },
    });
  }

  private registerObsidianEventListeners(): void {
    this.registerEvent(
      this.app.vault.on('create', (file: TAbstractFile) => {
        void this.controller?.getBridge()?.handleObsidianWrite(file);
      }),
    );
    this.registerEvent(
      this.app.vault.on('modify', (file: TAbstractFile) => {
        void this.controller?.getBridge()?.handleObsidianWrite(file);
      }),
    );
    this.registerEvent(
      this.app.vault.on('delete', (file: TAbstractFile) => {
        void this.controller?.getBridge()?.handleObsidianDelete(file);
      }),
    );
    this.registerEvent(
      this.app.vault.on('rename', (file: TAbstractFile, oldPath: string) => {
        void this.controller?.getBridge()?.handleObsidianRename(file, oldPath);
      }),
    );
  }
}

// Re-export for tests.
export type { TFile };
