// Owns the SDK Vault lifecycle and brokers state between everything else
// in the plugin. The state machine here is what the status bar visualizes
// and what the settings tab's Connect button toggles.
//
//   idle ──start──> connecting ──connected──> connected
//                                  └──error───> reconnecting ──→ connecting (loop)
//   any ──stop──> idle
//
// The controller does NOT directly listen for Obsidian events — that's the
// bridge's job. The controller wires the bridge's listeners during
// `start()` and tears them down during `stop()`.

import {
  type FileMeta,
  type IdentityInstance,
  type Label,
  Pubkey,
  type StorageAdapter,
  type TransportAdapter,
  Vault,
  type VaultEvent,
  type VaultInstance,
  initAgentsync,
} from '@agentsync/sdk/web-init';
import { type MinimalVault, ObsidianVaultBridge } from './bridge.js';
import { shouldSync } from './path-filter.js';
import { planReconcile } from './reconcile.js';
import type { AgentsyncSettings } from './settings.js';

export type ControllerState = 'idle' | 'connecting' | 'connected' | 'reconnecting' | 'error';

export interface ControllerDeps {
  storage: StorageAdapter;
  vault: MinimalVault;
  settings: AgentsyncSettings;
  /** The device identity, owned by the plugin (resolved from
   * `~/.agentsync/id_ed25519` on desktop). Passed straight into the SDK
   * Vault so the SDK never auto-generates or persists a seed itself. The
   * plugin frees it on unload — the controller must not. */
  identity: IdentityInstance;
  saveSettings: (s: AgentsyncSettings) => Promise<void>;
  /** Optional WASM bytes — if absent, callers must call initAgentsync() out-of-band. */
  wasmBytes?: Uint8Array;
  /** Optional pre-loaded Vault for tests; bypasses Vault.create/open. */
  sdkOverride?: VaultInstance;
  /**
   * Optional WebSocket transport. Defaults to the SDK's auto-detection
   * (`globalThis.WebSocket`). Node-side e2e tests inject `ws` here.
   */
  transport?: TransportAdapter;
  /** Test seam — emitter for status notices (avoid coupling to obsidian's Notice). */
  notice?: (msg: string) => void;
  log?: (msg: string) => void;
  /** Allow tests to inject their own clock for retry backoff. */
  now?: () => number;
}

type Listener = (s: ControllerState) => void;

export class SyncController {
  state: ControllerState = 'idle';
  private sdk: VaultInstance | null = null;
  private bridge: ObsidianVaultBridge | null = null;
  private listeners = new Set<Listener>();
  private unsubscribeSdk: (() => void) | null = null;
  private connectPromise: Promise<void> | null = null;

  constructor(private readonly deps: ControllerDeps) {}

  on(l: Listener): () => void {
    this.listeners.add(l);
    return () => this.listeners.delete(l);
  }

  private setState(s: ControllerState): void {
    if (this.state === s) return;
    this.state = s;
    for (const l of this.listeners) {
      try {
        l(s);
      } catch {
        // listener errors don't propagate
      }
    }
  }

  /** Wire-format public key to paste into the hub's authorized_keys.
   * Derived from the injected identity, so it's available immediately —
   * even before (or without) an SDK Vault / connection. */
  identityPubkeySsh(): string | null {
    const pk = this.deps.identity.pubkey();
    try {
      return pk.toSshString();
    } finally {
      pk.free();
    }
  }

  listLabels(): Label[] {
    return this.sdk?.listLabels() ?? [];
  }

  /**
   * Bring the controller online.
   *
   * - `prepare()` and `start({ connect: false })` are equivalent: they
   *   load the identity + open/create the SDK Vault + run a reconcile,
   *   but DON'T initiate a network connection. After this, the device's
   *   pubkey is available via `identityPubkeySsh()` so the user can add
   *   it to the hub's `authorized_keys` before connecting.
   * - `start({ connect: true })` (the default) does all of the above
   *   plus kicks off `connectWithReconnect`.
   *
   * Idempotent: calling start() again while the supervisor is already
   * running is a no-op. To force a fresh connection use `stop()` then
   * `start()`.
   */
  async start(opts: { connect?: boolean } = {}): Promise<void> {
    if (this.state !== 'idle' && this.state !== 'error') return;
    const wantConnect = (opts.connect ?? true) && this.deps.settings.rendezvousUrl !== '';

    if (this.deps.wasmBytes) {
      await initAgentsync(this.deps.wasmBytes);
    }

    if (wantConnect) this.setState('connecting');
    try {
      const sdk = this.deps.sdkOverride ?? (await this.openOrCreateVault());
      this.sdk = sdk;

      // Persist a fresh vaultId if one was just minted.
      if (!this.deps.settings.vaultId) {
        this.deps.settings.vaultId = sdk.vaultIdValue();
        await this.deps.saveSettings(this.deps.settings);
      }

      this.bridge = new ObsidianVaultBridge({
        vault: this.deps.vault,
        sdk,
        filter: (p) => shouldSync(p, this.deps.settings.ignoreGlobs),
        log: this.deps.log ?? (() => {}),
      });

      // Bring the two sides byte-equal before live events stream.
      await this.runReconcile();

      // Subscribe to SDK events so we update status + pull remote changes.
      this.unsubscribeSdk = sdk.subscribe((e) => this.onVaultEvent(e));

      if (!wantConnect) {
        this.setState('idle');
        return;
      }

      // Fire-and-forget the reconnect loop. It owns the connection
      // lifetime; we just listen to its emitted events to update state.
      this.connectPromise = sdk.connectWithReconnect({}).catch((err) => {
        this.deps.log?.(`reconnect supervisor exited with error: ${err}`);
      });
    } catch (err) {
      this.setState('error');
      this.deps.notice?.(`Agentsync: failed to start — ${err}`);
      throw err;
    }
  }

  /** Convenience: load identity + SDK without connecting. */
  async prepare(): Promise<void> {
    return this.start({ connect: false });
  }

  /** Disconnect and tear down. Safe to call from any state. */
  async stop(): Promise<void> {
    if (this.unsubscribeSdk) {
      this.unsubscribeSdk();
      this.unsubscribeSdk = null;
    }
    const sdk = this.sdk;
    this.bridge?.dispose();
    this.sdk = null;
    this.bridge = null;
    if (sdk) {
      try {
        await sdk.disconnect();
      } catch {}
      try {
        await sdk.close();
      } catch {}
    }
    this.connectPromise = null;
    this.setState('idle');
  }

  /** Run a full bidirectional reconcile pass. Useful as a recovery action. */
  async resyncNow(): Promise<void> {
    if (!this.sdk || !this.bridge) {
      this.deps.notice?.('Agentsync: not running.');
      return;
    }
    await this.runReconcile();
    this.deps.notice?.('Agentsync: resynced.');
  }

  async createLabel(name: string): Promise<void> {
    if (!this.sdk) return;
    await this.sdk.createLabel(name);
  }

  async restoreToLabel(name: string): Promise<void> {
    if (!this.sdk || !this.bridge) return;
    await this.sdk.restoreToLabel(name);
    await this.bridge.applyRemoteState();
  }

  /**
   * Wipe the local CRDT/sync state and stop the controller, then re-open
   * a fresh doc for the configured vault. Use when you change the vault
   * id, or to recover from a corrupt local doc.
   *
   * Cleared: `.agentsync/{doc.bin,sync-states,snapshots}`. **Kept:**
   * `config.toml` (so vault id / hub URL survive) and the device identity
   * (`~/.agentsync/id_ed25519` — it's shared with the CLI and outlives
   * any single vault; the same device key is reused). The Obsidian vault
   * contents are never touched.
   */
  async resetLocalState(): Promise<void> {
    await this.stop();
    // Zero-length bytes make loadDoc() return null, which the SDK treats
    // as "no doc" → the next start() re-creates/joins from config. The
    // identity is deliberately NOT touched.
    await this.deps.storage.saveDoc(new Uint8Array(0));
    await this.deps.storage.saveSnapshots(new Uint8Array(0));
    // Re-prepare immediately so the (unchanged) device pubkey stays
    // visible without forcing the user to click Connect.
    await this.prepare();
  }

  /** The bridge is exposed for the Obsidian event listeners in main.ts. */
  getBridge(): ObsidianVaultBridge | null {
    return this.bridge;
  }

  // ---- Internal ----

  private async openOrCreateVault(): Promise<VaultInstance> {
    const { rendezvousUrl, hubPubkey } = this.deps.settings;
    const transport = this.deps.transport;
    // An id in config is treated as an explicit pin. (Empty = "join
    // whatever this hub serves" — the common, recommended path.)
    const pinned = this.deps.settings.vaultId || '';
    const base = {
      storage: this.deps.storage,
      identity: this.deps.identity,
      ...(rendezvousUrl ? { rendezvousUrl } : {}),
      ...(hubPubkey ? { hubPubkey: sshPubkeyBytes(hubPubkey) } : {}),
      ...(transport ? { transport } : {}),
      name: 'obsidian',
    };

    // The hub mints and owns its vault id (the `agentsync clone` model),
    // so in connect-mode the HUB is authoritative — not config.toml, not
    // whatever doc.bin happens to be on disk. Probe it up front so a
    // stale local doc / stale config id can never feed the wrong id into
    // the handshake (that is the entire `vault_id mismatch` failure
    // class). Best-effort: if the hub is unreachable we fall back to the
    // local doc and let the reconnect supervisor retry (offline-friendly).
    let hubVaultId: string | null = null;
    if (rendezvousUrl) {
      try {
        const info = await Vault.probeHub({
          rendezvousUrl,
          ...(transport ? { transport } : {}),
        });
        hubVaultId = info.vaultId;
      } catch (e) {
        this.deps.log?.(`hub probe failed (${e}); falling back to local doc`);
      }
    }

    // Explicit pin that disagrees with the hub: fail loudly + actionably
    // rather than silently discarding a doc the user pinned an id for.
    if (pinned && hubVaultId && pinned !== hubVaultId) {
      throw new Error(
        `pinned vault id ${pinned} does not match the vault this hub serves (${hubVaultId}). Clear the Vault ID field to join the hub's vault automatically, or correct the pinned id.`,
      );
    }

    // Keep config.toml's id honest with what we'll actually be on.
    if (hubVaultId && this.deps.settings.vaultId !== hubVaultId) {
      this.deps.settings.vaultId = hubVaultId;
      await this.deps.saveSettings(this.deps.settings);
      this.deps.log?.(`joining hub vault ${hubVaultId}`);
    }

    const target = hubVaultId || pinned || null;
    const existing = await this.deps.storage.loadDoc();
    if (existing && existing.length > 0) {
      const sdk = await Vault.open(base);
      const local = sdk.vaultIdValue();
      if (!target || local === target) return sdk; // steady state
      // close() is flush-safe (it sets `closed` before flushing, so no
      // write) — safe to discard the stale doc right after.
      await sdk.close();
      if (hubVaultId) {
        // Connect-mode: the local doc is a different vault than the one
        // this hub serves (a leftover from an earlier wrong-id attempt).
        // Rebuild for the hub's vault. The user's notes live in the
        // Obsidian files (untouched) and re-sync on the next reconcile.
        this.deps.log?.(`local doc is vault ${local}; rebuilding for ${hubVaultId}`);
        await this.deps.storage.saveDoc(new Uint8Array(0));
        await this.deps.storage.saveSnapshots(new Uint8Array(0));
      } else {
        // Offline pin mismatch — no hub to arbitrate; keep the
        // actionable guard rather than guessing.
        throw new Error(
          `configured vault id ${target} does not match the local vault on disk (${local}). Run "Reset local state" in plugin settings to join ${target} from scratch.`,
        );
      }
    } else if (!target && !rendezvousUrl) {
      // Genuine offline create — Vault.create mints a fresh id.
    } else if (!target) {
      // Connect-mode but the hub was unreachable and we have no id to
      // fall back to: nothing actionable we can do.
      throw new Error(
        'could not reach the hub to discover its vault id — check the ' +
          'Hub URL and your connection, then try again.',
      );
    }

    return Vault.create({ ...base, ...(target ? { vaultId: target } : {}) });
  }

  private async runReconcile(): Promise<void> {
    if (!this.sdk || !this.bridge) return;
    const sdk = this.sdk;
    const bridge = this.bridge;
    const obsidianFiles = this.deps.vault.getFiles().map((f) => ({
      path: f.path,
      readText: () => this.deps.vault.read(f),
    }));
    const sdkFiles = sdk.listFiles().map((m: FileMeta) => ({
      path: m.path,
      deleted: !!m.deleted_at,
      readText: () => sdk.readTextFile(m.path),
    }));
    const plan = await planReconcile({
      obsidianFiles,
      sdkFiles,
      filter: (p) => shouldSync(p, this.deps.settings.ignoreGlobs),
    });
    // Yield to the event loop every YIELD_EVERY operations so a large
    // initial reconcile (hundreds of files in a fresh-pair scenario)
    // doesn't freeze the renderer. doc.bin is rewritten on every push, so
    // a 300-file vault is 300 atomic writes — IO-bound but tolerable as
    // long as we don't monopolize the main thread.
    const YIELD_EVERY = 25;
    let i = 0;
    for (const op of plan.pushToSdk) {
      await sdk.writeTextFile(op.path, op.content);
      bridge.pushed += 1;
      if (++i % YIELD_EVERY === 0) await new Promise((r) => setTimeout(r, 0));
    }
    let j = 0;
    for (const op of plan.applyToObsidian) {
      await bridge.applyOneRemoteFile({
        id: '',
        path: op.path,
        kind: 'Text',
        size: op.content.length,
        created_at: 0,
        updated_at: 0,
        deleted_at: null,
      });
      if (++j % YIELD_EVERY === 0) await new Promise((r) => setTimeout(r, 0));
    }
    let k = 0;
    for (const path of plan.deleteInObsidian) {
      const ex = this.deps.vault.getAbstractFileByPath(path);
      if (ex) {
        bridge.suppress(path);
        await this.deps.vault.delete(ex);
      }
      if (++k % YIELD_EVERY === 0) await new Promise((r) => setTimeout(r, 0));
    }
  }

  private onVaultEvent(e: VaultEvent): void {
    switch (e.kind) {
      case 'connecting':
        this.setState('connecting');
        break;
      case 'connected': {
        this.setState('connected');
        // TOFU-pin the hub identity in `[vault] hub_pubkey`, SSH wire
        // format — the same field/representation the native CLI uses.
        if (!this.deps.settings.hubPubkey) {
          const pk = Pubkey.fromBytes(e.hub_pubkey);
          try {
            this.deps.settings.hubPubkey = pk.toSshString();
          } finally {
            pk.free();
          }
          void this.deps.saveSettings(this.deps.settings);
        }
        break;
      }
      case 'disconnected':
        if (this.state !== 'idle') this.setState('reconnecting');
        break;
      case 'sync-progress':
        // Intentionally silent — too noisy for a status-bar transition.
        break;
      case 'doc-changed':
        // Coalesce bursts — initial sync against a large remote can fire
        // many of these in rapid succession.
        this.bridge?.scheduleApplyRemoteState();
        break;
      case 'error':
        this.setState('error');
        this.deps.notice?.(`Agentsync: ${e.message}`);
        break;
    }
  }
}

/** Decode an SSH-format pubkey (`[vault] hub_pubkey`) to raw bytes for
 * the SDK's pinned-hub option. Surfaces a readable error on a malformed
 * pin rather than a cryptic wasm panic. */
function sshPubkeyBytes(ssh: string): Uint8Array {
  let pk: ReturnType<typeof Pubkey.fromSshString>;
  try {
    pk = Pubkey.fromSshString(ssh);
  } catch (e) {
    throw new Error(`invalid [vault] hub_pubkey in config.toml: ${e}`);
  }
  try {
    return pk.bytes();
  } finally {
    pk.free();
  }
}

export function bytesToHex(bytes: Uint8Array): string {
  return Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');
}

export function hexToBytes(hex: string): Uint8Array {
  if (hex.length % 2 !== 0) throw new Error('hex string must have even length');
  const out = new Uint8Array(hex.length / 2);
  for (let i = 0; i < out.length; i++) {
    const byte = Number.parseInt(hex.slice(i * 2, i * 2 + 2), 16);
    if (Number.isNaN(byte)) throw new Error(`invalid hex at index ${i * 2}`);
    out[i] = byte;
  }
  return out;
}
