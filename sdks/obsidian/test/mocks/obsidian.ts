// Hand-rolled in-memory fakes for the slice of `obsidian` the plugin
// touches. Used by both unit and e2e tests; intentionally minimal — we
// match the production behaviors that matter for sync (event ordering,
// path normalization, atomicity) and ignore the rest.
//
// Plugin sources should accept these via dependency-injection. The Plugin
// base class is mocked too so main.ts can be exercised in tests.

import type { MinimalVault } from '../../src/bridge.js';
import type { MinimalDataAdapter } from '../../src/storage-adapter.js';

// ---------- TFile / TFolder / TAbstractFile ----------

export class FakeTAbstractFile {
  constructor(public path: string) {}

  get name(): string {
    const slash = this.path.lastIndexOf('/');
    return slash === -1 ? this.path : this.path.slice(slash + 1);
  }
}

export class FakeTFile extends FakeTAbstractFile {
  get extension(): string {
    const dot = this.name.lastIndexOf('.');
    return dot === -1 ? '' : this.name.slice(dot + 1);
  }
  get basename(): string {
    const dot = this.name.lastIndexOf('.');
    return dot === -1 ? this.name : this.name.slice(0, dot);
  }
}

export class FakeTFolder extends FakeTAbstractFile {
  children: FakeTAbstractFile[] = [];
}

// ---------- DataAdapter ----------

export class FakeDataAdapter implements MinimalDataAdapter {
  /** path → content (string for text, ArrayBuffer for binary) or null for dir. */
  private store = new Map<string, string | ArrayBuffer | null>();

  async read(path: string): Promise<string> {
    const v = this.store.get(path);
    if (v == null) throw new Error(`ENOENT: ${path}`);
    if (typeof v !== 'string') throw new Error(`not text: ${path}`);
    return v;
  }
  async readBinary(path: string): Promise<ArrayBuffer> {
    const v = this.store.get(path);
    if (v == null) throw new Error(`ENOENT: ${path}`);
    if (typeof v === 'string') {
      const bytes = new TextEncoder().encode(v);
      return bytes.buffer.slice(
        bytes.byteOffset,
        bytes.byteOffset + bytes.byteLength,
      ) as ArrayBuffer;
    }
    return v;
  }
  async write(path: string, data: string): Promise<void> {
    this.store.set(path, data);
  }
  async writeBinary(path: string, data: ArrayBuffer): Promise<void> {
    this.store.set(path, data.slice(0));
  }
  async exists(path: string): Promise<boolean> {
    return this.store.has(path);
  }
  async mkdir(path: string): Promise<void> {
    this.store.set(path, null);
  }
  async remove(path: string): Promise<void> {
    if (!this.store.has(path)) throw new Error(`ENOENT: ${path}`);
    this.store.delete(path);
  }
  async rename(oldPath: string, newPath: string): Promise<void> {
    if (!this.store.has(oldPath)) throw new Error(`ENOENT: ${oldPath}`);
    const v = this.store.get(oldPath);
    this.store.delete(oldPath);
    this.store.set(newPath, v ?? null);
  }

  /** Test helper — list everything stored. */
  entries(): Array<[string, string | ArrayBuffer | null]> {
    return Array.from(this.store.entries());
  }
}

// ---------- Vault + EventBus ----------

type VaultEventName = 'create' | 'modify' | 'delete' | 'rename';
type VaultEventHandler = (file: FakeTAbstractFile, oldPath?: string) => void;

export class FakeEventRef {
  constructor(
    public readonly name: VaultEventName,
    public readonly handler: VaultEventHandler,
  ) {}
}

export class FakeVault implements MinimalVault {
  private files = new Map<string, FakeTFile>();
  private folders = new Map<string, FakeTFolder>();
  private listeners = new Map<VaultEventName, Set<VaultEventHandler>>();

  constructor(public readonly adapter: FakeDataAdapter = new FakeDataAdapter()) {}

  // Obsidian's app.vault.configDir — points at the dot-prefixed config root.
  configDir = '.obsidian';

  // ---- Read APIs ----

  getFiles(): FakeTFile[] {
    return Array.from(this.files.values());
  }

  getMarkdownFiles(): FakeTFile[] {
    return this.getFiles().filter((f) => f.extension === 'md');
  }

  getAbstractFileByPath(path: string): FakeTAbstractFile | null {
    return this.files.get(path) ?? this.folders.get(path) ?? null;
  }

  // biome-ignore lint/suspicious/noExplicitAny: matches obsidian's signature
  async read(file: FakeTFile): Promise<string> {
    return this.adapter.read(file.path);
  }

  // ---- Write APIs ----

  async create(path: string, data: string): Promise<FakeTFile> {
    if (this.files.has(path)) throw new Error(`exists: ${path}`);
    await this.adapter.write(path, data);
    const f = new FakeTFile(path);
    this.files.set(path, f);
    this.emit('create', f);
    return f;
  }

  async modify(file: FakeTFile, data: string): Promise<void> {
    if (!this.files.has(file.path)) throw new Error(`not found: ${file.path}`);
    await this.adapter.write(file.path, data);
    this.emit('modify', file);
  }

  async delete(file: FakeTAbstractFile): Promise<void> {
    if (this.files.has(file.path)) {
      this.files.delete(file.path);
      try {
        await this.adapter.remove(file.path);
      } catch {}
    } else if (this.folders.has(file.path)) {
      this.folders.delete(file.path);
    } else {
      throw new Error(`not found: ${file.path}`);
    }
    this.emit('delete', file);
  }

  async rename(file: FakeTAbstractFile, newPath: string): Promise<void> {
    const oldPath = file.path;
    if (this.files.has(oldPath)) {
      const f = this.files.get(oldPath)!;
      this.files.delete(oldPath);
      f.path = newPath;
      this.files.set(newPath, f);
      try {
        await this.adapter.rename(oldPath, newPath);
      } catch {}
      this.emit('rename', f, oldPath);
    } else if (this.folders.has(oldPath)) {
      const f = this.folders.get(oldPath)!;
      this.folders.delete(oldPath);
      f.path = newPath;
      this.folders.set(newPath, f);
      this.emit('rename', f, oldPath);
    } else {
      throw new Error(`not found: ${oldPath}`);
    }
  }

  async createFolder(path: string): Promise<FakeTFolder> {
    if (this.folders.has(path)) return this.folders.get(path)!;
    const f = new FakeTFolder(path);
    this.folders.set(path, f);
    return f;
  }

  // ---- Events ----

  on(name: VaultEventName, handler: VaultEventHandler): FakeEventRef {
    let set = this.listeners.get(name);
    if (!set) {
      set = new Set();
      this.listeners.set(name, set);
    }
    set.add(handler);
    return new FakeEventRef(name, handler);
  }

  offref(ref: FakeEventRef): void {
    this.listeners.get(ref.name)?.delete(ref.handler);
  }

  private emit(name: VaultEventName, file: FakeTAbstractFile, oldPath?: string): void {
    const set = this.listeners.get(name);
    if (!set) return;
    for (const h of [...set]) {
      try {
        h(file, oldPath);
      } catch {
        // Listener errors don't propagate — matches Obsidian's behavior.
      }
    }
  }

  /** Test helper — number of registered listeners across all events. */
  listenerCount(): number {
    let n = 0;
    for (const set of this.listeners.values()) n += set.size;
    return n;
  }
}

// ---------- App ----------

export class FakeApp {
  vault: FakeVault;
  setting = {
    open: () => {},
    openTabById: (_id: string) => {},
  };

  constructor(vault?: FakeVault) {
    this.vault = vault ?? new FakeVault();
  }
}

// ---------- Plugin / Notice / Modal ----------

export interface FakePluginManifest {
  id: string;
  name: string;
  version: string;
}

export class FakePlugin {
  app: FakeApp;
  manifest: FakePluginManifest;
  private data: unknown = null;
  private commands: Array<{ id: string; name: string; cb: () => void }> = [];
  private statusItems: HTMLElement[] = [];
  private settingTabs: unknown[] = [];
  private events: FakeEventRef[] = [];

  constructor(app: FakeApp, manifest: FakePluginManifest = defaultManifest()) {
    this.app = app;
    this.manifest = manifest;
  }

  async loadData(): Promise<unknown> {
    return this.data;
  }

  async saveData(d: unknown): Promise<void> {
    this.data = d;
  }

  registerEvent(ref: FakeEventRef): void {
    this.events.push(ref);
  }

  addCommand(c: { id: string; name: string; callback: () => void }): void {
    this.commands.push({ id: c.id, name: c.name, cb: c.callback });
  }

  addStatusBarItem(): HTMLElement {
    const el = makeFakeHTMLElement();
    this.statusItems.push(el);
    return el;
  }

  addSettingTab(tab: unknown): void {
    this.settingTabs.push(tab);
  }

  /** Test helper — invoke a registered command by id. */
  invokeCommand(id: string): void {
    const c = this.commands.find((x) => x.id === id);
    if (!c) throw new Error(`no such command: ${id}`);
    c.cb();
  }

  /** Test helper — list registered command ids. */
  commandIds(): string[] {
    return this.commands.map((c) => c.id);
  }

  /** Test helper — get the most recently-added status bar item. */
  lastStatusBarItem(): HTMLElement | null {
    return this.statusItems[this.statusItems.length - 1] ?? null;
  }

  /** Test helper — fire onunload to release events. */
  async simulateUnload(): Promise<void> {
    for (const ref of this.events) {
      this.app.vault.offref(ref);
    }
    this.events = [];
  }
}

function defaultManifest(): FakePluginManifest {
  return { id: 'agentsync-sync', name: 'Agentsync', version: '0.0.0' };
}

export const noticeLog: string[] = [];
export class FakeNotice {
  constructor(public message: string) {
    noticeLog.push(message);
  }
}

// ---------- Minimal HTMLElement shim for status bar / DOM ops ----------

interface FakeElementMethods {
  setText(t: string): void;
  addClass(c: string): void;
  removeClass(c: string): void;
  hasClass(c: string): boolean;
  empty(): void;
}

export type FakeHTMLElement = HTMLElement & FakeElementMethods;

export function makeFakeHTMLElement(): FakeHTMLElement {
  const classes = new Set<string>();
  let text = '';
  const handlers = new Map<string, Set<(e: Event) => void>>();
  const el: Partial<FakeHTMLElement> & { _classes: Set<string>; _text(): string } = {
    _classes: classes,
    _text: () => text,
    setText(t: string) {
      text = t;
    },
    addClass(c: string) {
      classes.add(c);
    },
    removeClass(c: string) {
      classes.delete(c);
    },
    hasClass(c: string) {
      return classes.has(c);
    },
    empty() {
      text = '';
      classes.clear();
    },
    addEventListener(name: string, h: (e: Event) => void) {
      let s = handlers.get(name);
      if (!s) {
        s = new Set();
        handlers.set(name, s);
      }
      s.add(h);
    },
    removeEventListener(name: string, h: (e: Event) => void) {
      handlers.get(name)?.delete(h);
    },
    dispatchEvent(e: Event) {
      const set = handlers.get(e.type);
      if (set) {
        for (const h of set) h(e);
      }
      return true;
    },
  };
  return el as FakeHTMLElement;
}

/** Reset all module-level capture buffers. Call in `beforeEach`. */
export function resetMocks(): void {
  noticeLog.length = 0;
}
