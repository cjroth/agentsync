// Two-way sync between Obsidian's `app.vault` and the SDK's `Vault`.
//
//   Obsidian → SDK : listens for create/modify/delete/rename events and
//                    pushes changes via vault.write/delete/renameTextFile().
//   SDK → Obsidian : listens for `doc-changed` events from the SDK and
//                    walks `vault.listFiles()` to apply diffs.
//
// Feedback-loop suppression: every plugin-originated write registers the
// path in `suppressed` for one event tick; the modify/create/delete
// handlers consume the token and bail. As a second line of defense, we
// also content-compare before pushing — if the bytes are already equal we
// skip the push, which catches anything the suppression set missed (e.g.
// Obsidian re-saving a file with identical content).

import type { FileMeta, VaultInstance } from '@agentsync/sdk/web-init';

// We avoid importing types from `obsidian` directly here because the npm
// package ships only type declarations (no runtime). Tests run plugin
// modules under Bun and only stub the parts they use; pulling the full
// `TFile`/`TAbstractFile` types in would force every test fixture to mimic
// fields (`stat`, `vault`, `parent`) the bridge never reads. Instead we
// declare the structural slice we actually need.

/** Structural shape of an Obsidian abstract file (file or folder). */
export interface MinimalAbstractFile {
  path: string;
  name: string;
}

/** Structural shape of an Obsidian text file. */
export interface MinimalFile extends MinimalAbstractFile {
  extension: string;
}

/** Structural shape of `App.vault` — exactly the methods the bridge calls. */
export interface MinimalVault {
  getFiles(): MinimalFile[];
  getAbstractFileByPath(path: string): MinimalAbstractFile | null;
  read(file: MinimalFile): Promise<string>;
  create(path: string, data: string): Promise<MinimalFile>;
  modify(file: MinimalFile, data: string): Promise<void>;
  delete(file: MinimalAbstractFile, force?: boolean): Promise<void>;
  rename(file: MinimalAbstractFile, newPath: string): Promise<void>;
  createFolder(path: string): Promise<unknown>;
}

export interface BridgeDeps {
  vault: MinimalVault;
  sdk: VaultInstance;
  filter: (path: string) => boolean;
  log?: (msg: string) => void;
  /** Test seam — whether `f` is a file (not a folder). */
  isFile?: (f: MinimalAbstractFile | null) => f is MinimalFile;
}

const defaultIsFile = (f: MinimalAbstractFile | null): f is MinimalFile =>
  !!f && (f as MinimalFile).extension !== undefined;

/** Yield control back to the event loop so the renderer can paint. */
function yieldToEventLoop(): Promise<void> {
  return new Promise((r) => setTimeout(r, 0));
}

export class ObsidianVaultBridge {
  /** Paths the bridge wrote — modify/create handlers eat one token each. */
  private suppressed = new Map<string, number>();
  /**
   * Snapshot of alive SDK paths from the last `applyRemoteState` call.
   * `Doc.listFiles()` filters tombstones, so the only signal that a remote
   * file was deleted is its disappearance from this set on the next call.
   */
  private knownSdkPaths = new Set<string>();
  /** Counter of pushed changes since `start()` — exposed for tests + status. */
  pushed = 0;
  pulled = 0;
  skipped = 0;

  private isFile: (f: MinimalAbstractFile | null) => f is MinimalFile;
  private log: (msg: string) => void;
  /** Coalesce bursts of `doc-changed` events — initial sync against a
   * large remote can fire dozens of them in a row; one applyRemoteState
   * pass per burst is plenty. */
  private applyTimer: ReturnType<typeof setTimeout> | null = null;
  private applyPending = false;
  private applyInFlight = false;
  /** Set in `dispose()` — gates the debounce so a late timer firing
   * after the controller stopped doesn't poke a freed wasm Doc. */
  private disposed = false;
  /** How often to yield to the event loop while iterating large vaults. */
  private static readonly YIELD_EVERY = 25;
  private static readonly APPLY_DEBOUNCE_MS = 200;

  constructor(private readonly deps: BridgeDeps) {
    this.isFile = deps.isFile ?? defaultIsFile;
    this.log = deps.log ?? (() => {});
  }

  /** Suppress the next event for `path`. Public for the executor. */
  suppress(path: string): void {
    this.suppressed.set(path, (this.suppressed.get(path) ?? 0) + 1);
  }

  /**
   * Returns true if `path` was suppressed (and consumes one token);
   * the handler should bail.
   */
  consumeSuppression(path: string): boolean {
    const n = this.suppressed.get(path);
    if (!n) return false;
    if (n === 1) this.suppressed.delete(path);
    else this.suppressed.set(path, n - 1);
    return true;
  }

  // ---- Obsidian → SDK ----

  /** Handle a create or modify event from Obsidian. */
  async handleObsidianWrite(file: MinimalAbstractFile): Promise<void> {
    if (!this.isFile(file)) return;
    if (this.consumeSuppression(file.path)) return;
    if (!this.deps.filter(file.path)) {
      this.skipped += 1;
      this.log(`skip (filter): ${file.path}`);
      return;
    }
    const content = await this.deps.vault.read(file);
    // Equality short-circuit — avoids generating a redundant Automerge op
    // when the suppression set missed (e.g. re-save with identical bytes).
    if (this.deps.sdk.fileExists(file.path)) {
      const remote = await this.deps.sdk.readTextFile(file.path);
      if (remote === content) return;
    }
    await this.deps.sdk.writeTextFile(file.path, content);
    this.pushed += 1;
    this.log(`push: ${file.path} (${content.length}B)`);
  }

  /** Handle a delete event from Obsidian. */
  async handleObsidianDelete(file: MinimalAbstractFile): Promise<void> {
    if (this.consumeSuppression(file.path)) return;
    // Deletes still go through even if the path filter would normally skip
    // them — the SDK should learn the file is gone if it was ever synced.
    // But if the SDK doesn't know about it, there's nothing to delete.
    if (!this.deps.sdk.fileExists(file.path)) return;
    await this.deps.sdk.deleteFile(file.path);
    this.pushed += 1;
    this.log(`delete: ${file.path}`);
  }

  /** Handle a rename event from Obsidian. */
  async handleObsidianRename(file: MinimalAbstractFile, oldPath: string): Promise<void> {
    if (this.consumeSuppression(file.path)) return;
    const fromAllowed = this.deps.filter(oldPath);
    const toAllowed = this.deps.filter(file.path);

    if (fromAllowed && toAllowed) {
      if (this.deps.sdk.fileExists(oldPath)) {
        await this.deps.sdk.renameFile(oldPath, file.path);
      } else if (this.isFile(file)) {
        const content = await this.deps.vault.read(file);
        await this.deps.sdk.writeTextFile(file.path, content);
      }
      this.pushed += 1;
      return;
    }

    if (fromAllowed && !toAllowed) {
      if (this.deps.sdk.fileExists(oldPath)) {
        await this.deps.sdk.deleteFile(oldPath);
        this.pushed += 1;
      }
      return;
    }

    if (!fromAllowed && toAllowed && this.isFile(file)) {
      const content = await this.deps.vault.read(file);
      await this.deps.sdk.writeTextFile(file.path, content);
      this.pushed += 1;
      return;
    }
    // Neither side allowed — nothing to do.
  }

  // ---- SDK → Obsidian ----

  /**
   * Schedule an `applyRemoteState` pass with leading-edge debounce. The
   * SDK can emit many `doc-changed` events in a burst during initial
   * sync; this collapses them so we run at most one full pass per
   * APPLY_DEBOUNCE_MS window plus one trailing pass.
   */
  scheduleApplyRemoteState(): void {
    if (this.disposed) return;
    if (this.applyInFlight) {
      this.applyPending = true;
      return;
    }
    if (this.applyTimer !== null) {
      this.applyPending = true;
      return;
    }
    this.applyTimer = setTimeout(() => {
      this.applyTimer = null;
      if (this.disposed) return;
      void this.runScheduledApply();
    }, ObsidianVaultBridge.APPLY_DEBOUNCE_MS);
  }

  private async runScheduledApply(): Promise<void> {
    if (this.disposed) return;
    this.applyInFlight = true;
    try {
      await this.applyRemoteState();
    } finally {
      this.applyInFlight = false;
    }
    if (this.applyPending && !this.disposed) {
      this.applyPending = false;
      this.scheduleApplyRemoteState();
    }
  }

  /** Tear down the debounce machinery. Called by the controller's stop()
   * before the SDK Doc is freed — guarantees no late timer can deref it. */
  dispose(): void {
    this.disposed = true;
    if (this.applyTimer !== null) {
      clearTimeout(this.applyTimer);
      this.applyTimer = null;
    }
    this.applyPending = false;
  }

  /**
   * Apply the SDK's current state to the Obsidian vault. Detects remote
   * deletions by diffing against the previous live snapshot — files that
   * were alive last call but missing now are treated as tombstoned.
   *
   * Yields to the event loop every YIELD_EVERY files so a large initial
   * sync doesn't freeze the renderer.
   */
  async applyRemoteState(): Promise<void> {
    const currentPaths = new Set<string>();
    const sdkFiles = this.deps.sdk.listFiles();
    let i = 0;
    for (const meta of sdkFiles) {
      if (meta.kind === 'Text' && this.deps.filter(meta.path)) {
        currentPaths.add(meta.path);
      }
      await this.applyOneRemoteFile(meta);
      if (++i % ObsidianVaultBridge.YIELD_EVERY === 0) await yieldToEventLoop();
    }
    // Apply tombstones inferred from the diff.
    let j = 0;
    for (const path of this.knownSdkPaths) {
      if (currentPaths.has(path)) continue;
      if (!this.deps.filter(path)) continue;
      const ex = this.deps.vault.getAbstractFileByPath(path);
      if (!ex) continue;
      this.suppress(path);
      await this.deps.vault.delete(ex);
      this.pulled += 1;
      this.log(`pull-delete (tombstone): ${path}`);
      if (++j % ObsidianVaultBridge.YIELD_EVERY === 0) await yieldToEventLoop();
    }
    this.knownSdkPaths = currentPaths;
  }

  /** Apply a single remote file (called from `applyRemoteState` and tests). */
  async applyOneRemoteFile(meta: FileMeta): Promise<void> {
    if (meta.kind !== 'Text') return;
    if (!this.deps.filter(meta.path)) return;

    const existing = this.deps.vault.getAbstractFileByPath(meta.path);

    if (meta.deleted_at) {
      if (existing) {
        this.suppress(meta.path);
        await this.deps.vault.delete(existing);
        this.pulled += 1;
        this.log(`pull-delete: ${meta.path}`);
      }
      return;
    }

    const content = await this.deps.sdk.readTextFile(meta.path);

    if (existing && this.isFile(existing)) {
      const cur = await this.deps.vault.read(existing);
      if (cur === content) return;
      this.suppress(meta.path);
      await this.deps.vault.modify(existing, content);
      this.pulled += 1;
      this.log(`pull-modify: ${meta.path}`);
      return;
    }

    // Create — ensure parent folders exist first.
    await this.ensureFolderFor(meta.path);
    this.suppress(meta.path);
    try {
      await this.deps.vault.create(meta.path, content);
    } catch (e) {
      // Cold metadata cache on reopen: the file physically exists (a
      // prior session's sync) but getAbstractFileByPath returned null, so
      // we took the create path and Obsidian throws "File already
      // exists." Recover by writing the remote content into the existing
      // file rather than aborting the whole reconcile. Real failures
      // (I/O, permissions) still propagate.
      if (!/already exists/i.test(String((e as Error)?.message ?? e))) throw e;
      const f = this.deps.vault.getAbstractFileByPath(meta.path);
      if (f && this.isFile(f)) {
        await this.deps.vault.modify(f, content);
      } else {
        // Still unresolved (cache truly cold) — don't crash the sync; a
        // later reconcile pass with a warm cache reconciles it.
        this.log(`pull-create: ${meta.path} exists but unresolved (cold cache); deferring`);
        return;
      }
    }
    this.pulled += 1;
    this.log(`pull-create: ${meta.path}`);
  }

  /** Ensure all ancestor folders for `filePath` exist in the vault. */
  async ensureFolderFor(filePath: string): Promise<void> {
    const slash = filePath.lastIndexOf('/');
    if (slash <= 0) return;
    const folder = filePath.slice(0, slash);
    if (this.deps.vault.getAbstractFileByPath(folder)) return;
    const parts = folder.split('/').filter(Boolean);
    let cur = '';
    for (const seg of parts) {
      cur = cur ? `${cur}/${seg}` : seg;
      if (this.deps.vault.getAbstractFileByPath(cur)) continue;
      try {
        await this.deps.vault.createFolder(cur);
      } catch (e) {
        // Obsidian's metadata cache isn't warm right after launch, so a
        // folder that physically exists (from a prior session's sync)
        // looks absent via getAbstractFileByPath and createFolder() then
        // throws "Folder already exists." That race is benign — the
        // folder is there, which is all ensureFolderFor needs. Any other
        // failure (I/O, permissions) must still propagate.
        if (!/already exists/i.test(String((e as Error)?.message ?? e))) throw e;
      }
    }
  }
}
