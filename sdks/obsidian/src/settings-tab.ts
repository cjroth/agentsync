// PluginSettingTab UI. Imports the live `obsidian` package so this module
// only loads inside the host runtime — never from unit tests, which use
// the pure schema in `./settings.js`.
//
// Two states:
//   - Unconfigured → a setup wizard (Create new / Connect to existing).
//     This is the ONLY way `.agentsync/` gets created.
//   - Configured → the sync toggle + editable config + snapshots.

import { type App, type ButtonComponent, Notice, PluginSettingTab, Setting } from 'obsidian';
import type AgentsyncPlugin from './main.js';
import { parseIgnoreGlobs } from './settings.js';

export class AgentsyncSettingTab extends PluginSettingTab {
  private unsubscribe: (() => void) | null = null;
  /** Coalesces controller-driven redraws so a flapping reconnect doesn't
   * trigger a render per state transition. */
  private redrawQueued = false;

  // Transient wizard state (not persisted until the user submits setup).
  private setupMode: 'create' | 'connect' = 'create';
  private setupName = '';
  private setupVaultId = '';
  private setupHubUrl = '';
  private busy = false;
  /** Seed the wizard from a saved-but-incomplete config exactly once, so
   * a failed attempt is pre-filled but in-progress edits aren't clobbered
   * by redraws (dropdown changes call display() repeatedly). */
  private seeded = false;

  constructor(
    app: App,
    private readonly plugin: AgentsyncPlugin,
  ) {
    super(app, plugin);
  }

  override hide(): void {
    this.unsubscribe?.();
    this.unsubscribe = null;
  }

  override display(): void {
    this.unsubscribe?.();
    this.unsubscribe = null;
    const { containerEl } = this;
    containerEl.empty();
    containerEl.createEl('h2', { text: 'Agentsync' });

    if (!this.plugin.isConfigured()) {
      this.renderSetup(containerEl);
      return;
    }
    this.renderConfigured(containerEl);
  }

  // ---- Unconfigured: setup wizard ----

  private renderSetup(containerEl: HTMLElement): void {
    if (!this.seeded) {
      const s = this.plugin.settings;
      // Pick the likely mode, but deliberately do NOT pre-fill the Vault
      // ID: it's the optional/advanced pin field, and a not-yet-onboarded
      // config id is usually a stale leftover. Leaving it blank steers
      // the user onto the recommended auto-discovery path.
      if (s.vaultId || (s.rendezvousUrl && !s.vaultName)) {
        this.setupMode = 'connect';
      } else if (s.vaultName) {
        this.setupMode = 'create';
        this.setupName = s.vaultName;
      }
      if (s.rendezvousUrl) this.setupHubUrl = s.rendezvousUrl;
      this.seeded = true;
    }

    containerEl.createEl('p', {
      text:
        'This vault is not set up yet. Setup writes ' +
        '.agentsync/config.toml, but syncing only activates once setup ' +
        'completes — building the local vault, or (when connecting) ' +
        'reaching the hub.',
    });

    if (this.plugin.onboardingError) {
      containerEl
        .createEl('p', {
          text: `Last attempt failed: ${this.plugin.onboardingError}`,
        })
        .addClass('mod-warning');
    }

    new Setting(containerEl)
      .setName('Setup mode')
      .setDesc('Create a brand-new vault, or connect this device to an existing one.')
      .addDropdown((d) =>
        d
          .addOption('create', 'Create a new vault')
          .addOption('connect', 'Connect to an existing vault')
          .setValue(this.setupMode)
          .onChange((v) => {
            this.setupMode = v as 'create' | 'connect';
            this.display();
          }),
      );

    if (this.setupMode === 'create') {
      new Setting(containerEl)
        .setName('Vault name')
        .setDesc('Optional label, shared with the CLI as `[vault] name`.')
        .addText((t) =>
          t
            .setPlaceholder('My Notes')
            .setValue(this.setupName)
            .onChange((v) => {
              this.setupName = v;
            }),
        );
      new Setting(containerEl)
        .setName('Hub URL')
        .setDesc('Optional now — you can add it later before syncing across devices.')
        .addText((t) =>
          t
            .setPlaceholder('wss://hub.example.com:7777')
            .setValue(this.setupHubUrl)
            .onChange((v) => {
              this.setupHubUrl = v;
            }),
        );
    } else {
      new Setting(containerEl)
        .setName('Hub URL')
        .setDesc('Required — the hub that hosts the existing vault.')
        .addText((t) =>
          t
            .setPlaceholder('wss://hub.example.com:7777')
            .setValue(this.setupHubUrl)
            .onChange((v) => {
              this.setupHubUrl = v;
            }),
        );
      new Setting(containerEl)
        .setName('Vault ID')
        .setDesc(
          'Optional — discovered from the hub automatically. Set this ' +
            'only to pin a specific vault id (the connect will fail if ' +
            "it doesn't match what the hub serves).",
        )
        .addText((t) =>
          t
            .setPlaceholder('(auto-discovered)')
            .setValue(this.setupVaultId)
            .onChange((v) => {
              this.setupVaultId = v;
            }),
        );
    }

    new Setting(containerEl).addButton((b) =>
      b
        .setButtonText(this.busy ? 'Setting up…' : 'Set up Agentsync')
        .setCta()
        .setDisabled(this.busy)
        .onClick(async () => {
          if (this.setupMode === 'connect' && !this.setupHubUrl.trim()) {
            new Notice('Agentsync: Hub URL is required to connect.');
            return;
          }
          this.busy = true;
          this.display();
          try {
            await this.plugin.runSetup({
              mode: this.setupMode,
              vaultName: this.setupName,
              vaultId: this.setupVaultId,
              rendezvousUrl: this.setupHubUrl,
            });
            new Notice('Agentsync: setup complete.');
          } catch (err) {
            new Notice(`Agentsync: setup failed — ${err}`);
          } finally {
            this.busy = false;
            this.display();
          }
        }),
    );
  }

  // ---- Configured: normal settings ----

  private renderConfigured(containerEl: HTMLElement): void {
    // Keep this view live: re-render on any controller state change so a
    // dropped/failed connection (or the pubkey becoming available) is
    // reflected immediately instead of a stale snapshot. Deferred so we
    // don't re-enter display() from inside the controller's listener
    // iteration; coalesced so a flapping reconnect doesn't storm renders.
    // Torn down by display()/hide() via this.unsubscribe.
    this.unsubscribe =
      this.plugin.controller?.on(() => {
        if (this.redrawQueued) return;
        this.redrawQueued = true;
        setTimeout(() => {
          this.redrawQueued = false;
          this.display();
        }, 0);
      }) ?? null;

    new Setting(containerEl)
      .setName('Enable sync')
      .setDesc(
        'Master switch. While off, the plugin makes no connection and ' +
          'opens no vault. Turn off to pause syncing without losing config.',
      )
      .addToggle((t) =>
        t.setValue(this.plugin.settings.syncEnabled).onChange(async (v) => {
          await this.plugin.setSyncEnabled(v);
          this.display();
        }),
      );

    const pubkey = this.plugin.controller?.identityPubkeySsh() ?? '(loading…)';
    new Setting(containerEl)
      .setName('Device public key')
      .setDesc('Add this line to your hub’s `authorized_keys` to authorize this device.')
      .addText((t) => t.setValue(pubkey).setDisabled(true))
      .addButton((b: ButtonComponent) =>
        b
          .setButtonText('Copy')
          .setTooltip('Copy public key to clipboard')
          .onClick(async () => {
            const ssh = this.plugin.controller?.identityPubkeySsh();
            if (ssh) await navigator.clipboard.writeText(ssh);
          }),
      );

    new Setting(containerEl)
      .setName('Hub URL')
      .setDesc('WebSocket URL of your agentsync hub, e.g. `wss://hub.example.com:7777`.')
      .addText((t) =>
        t
          .setPlaceholder('wss://hub.example.com:7777')
          .setValue(this.plugin.settings.rendezvousUrl)
          .onChange(async (v) => {
            this.plugin.settings.rendezvousUrl = v.trim();
            await this.plugin.saveSettings();
          }),
      );

    new Setting(containerEl)
      .setName('Vault ID')
      .setDesc(
        'Stored in .agentsync/config.toml. If you change this, run ' +
          '“Reset local state” below so the local doc is rebuilt for the ' +
          'new vault.',
      )
      .addText((t) =>
        t
          .setPlaceholder('00000000-0000-4000-8000-000000000000')
          .setValue(this.plugin.settings.vaultId)
          .onChange(async (v) => {
            this.plugin.settings.vaultId = v.trim();
            await this.plugin.saveSettings();
          }),
      );

    new Setting(containerEl)
      .setName('Auto-connect on start')
      .setDesc('Open the hub connection automatically when Obsidian launches.')
      .addToggle((t) =>
        t.setValue(this.plugin.settings.autoConnectOnStart).onChange(async (v) => {
          this.plugin.settings.autoConnectOnStart = v;
          await this.plugin.saveSettings();
        }),
      );

    new Setting(containerEl)
      .setName('Ignore patterns')
      .setDesc(
        'One glob per line. Lines starting with `#` are comments. ' +
          'Binary files (images, PDFs, …) are always skipped.',
      )
      .addTextArea((t) =>
        t
          .setPlaceholder('# example\nDrafts/**\n*.tmp.md')
          .setValue(this.plugin.settings.ignoreGlobs.join('\n'))
          .onChange(async (v) => {
            this.plugin.settings.ignoreGlobs = parseIgnoreGlobs(v);
            await this.plugin.saveSettings();
          }),
      );

    const pinned = this.plugin.settings.hubPubkey || '(none yet)';
    new Setting(containerEl)
      .setName('Pinned hub key')
      .setDesc(
        'Set on first successful connect (TOFU), stored as ' +
          '`[vault] hub_pubkey`. Clear to allow connecting to a different hub.',
      )
      .addText((t) => t.setValue(pinned).setDisabled(true))
      .addButton((b) =>
        b
          .setButtonText('Clear pin')
          .setWarning()
          .onClick(async () => {
            this.plugin.settings.hubPubkey = '';
            await this.plugin.saveSettings();
            this.display();
          }),
      );

    const state = this.plugin.controller?.state ?? 'idle';
    const connected = state === 'connected';
    new Setting(containerEl)
      .setName('Connection')
      .setDesc(`Current state: ${state}.`)
      .addButton((b) =>
        b
          // Only a real connection shows "Disconnect"; anything else
          // (idle / connecting / reconnecting / error) offers a single
          // click to (re)connect — no Disconnect-first dance.
          .setButtonText(connected ? 'Disconnect' : state === 'idle' ? 'Connect' : 'Reconnect')
          .setCta()
          .onClick(async () => {
            // Always stop first: start() no-ops while a reconnect
            // supervisor is alive, so a stuck "reconnecting" loop must be
            // cleared before a fresh connect can take.
            await this.plugin.controller?.stop();
            if (connected) await this.plugin.controller?.prepare();
            else await this.plugin.controller?.start({ connect: true });
            this.display();
          }),
      )
      .addButton((b) =>
        b
          .setButtonText('Resync now')
          .setTooltip(
            'Re-runs the bidirectional reconcile pass — pulls anything new ' +
              'from the hub into Obsidian and pushes anything new from ' +
              'Obsidian to the hub.',
          )
          .onClick(async () => {
            await this.plugin.controller?.resyncNow();
          }),
      );

    new Setting(containerEl)
      .setName('Reset local state')
      .setDesc(
        'Rebuilds .agentsync/{doc.bin, sync-states}; config.toml and your ' +
          'device key are kept (the key lives in ~/.agentsync and is shared ' +
          'with the CLI). Your Obsidian vault contents are NOT touched. Use ' +
          'after changing the Vault ID, or to recover a corrupt local doc.',
      )
      .addButton((b) =>
        b
          .setButtonText('Reset')
          .setWarning()
          .onClick(async () => {
            await this.plugin.controller?.resetLocalState();
            this.plugin.settings.hubPubkey = '';
            await this.plugin.saveSettings();
            new Notice('Agentsync: local state cleared.');
            this.display();
          }),
      );

    containerEl.createEl('h3', { text: 'Snapshots' });
    const labels = this.plugin.controller?.listLabels() ?? [];
    if (labels.length === 0) {
      containerEl.createEl('p', {
        text: 'No snapshots yet. Snapshots are point-in-time labels you can restore.',
      });
    } else {
      for (const label of labels) {
        const created = new Date(label.created_at_ms).toLocaleString();
        new Setting(containerEl)
          .setName(label.name)
          .setDesc(`Created ${created}`)
          .addButton((b) =>
            b
              .setButtonText('Restore')
              .setWarning()
              .onClick(async () => {
                await this.plugin.controller?.restoreToLabel(label.name);
                this.display();
              }),
          );
      }
    }

    new Setting(containerEl).setName('Create snapshot').addButton((b) =>
      b
        .setButtonText('Create')
        .setCta()
        .onClick(async () => {
          const name = `snapshot-${new Date().toISOString().replace(/[:.]/g, '-')}`;
          await this.plugin.controller?.createLabel(name);
          this.display();
        }),
    );
  }
}
