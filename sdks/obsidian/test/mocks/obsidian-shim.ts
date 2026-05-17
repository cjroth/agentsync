// Runtime stub for the `obsidian` npm package, which ships only type
// declarations and would error at import time inside Bun. We re-export
// minimal class shapes that make the production code (Plugin / Modal /
// Setting / etc.) executable inside tests.
//
// `test/setup.ts` registers this via `Bun.plugin` so any test that imports
// (transitively) from a plugin source file gets these implementations
// instead of the empty `obsidian` package.

export class Notice {
  static log: string[] = [];
  constructor(public message: string) {
    Notice.log.push(message);
  }
}

export class App {
  vault: any;
  setting = { open: () => {}, openTabById: (_: string) => {} };
  constructor(vault?: any) {
    this.vault = vault ?? {};
  }
}

class _ComponentBase {
  registerEvent(_e: unknown): void {}
  registerInterval(_i: unknown): number {
    return 0;
  }
  registerDomEvent(_t: unknown, _ev: string, _h: unknown): void {}
}

export class Plugin extends _ComponentBase {
  app: App;
  manifest: { id: string; name: string; version: string };
  settingTabs: unknown[] = [];
  commands: Array<{ id: string; name: string; cb: () => void }> = [];
  statusBarItems: unknown[] = [];
  private data: unknown = null;

  constructor(app: App, manifest: { id: string; name: string; version: string }) {
    super();
    this.app = app;
    this.manifest = manifest;
  }

  async loadData(): Promise<unknown> {
    return this.data;
  }
  async saveData(d: unknown): Promise<void> {
    this.data = d;
  }
  addCommand(c: { id: string; name: string; callback: () => void }): void {
    this.commands.push({ id: c.id, name: c.name, cb: c.callback });
  }
  addStatusBarItem(): unknown {
    const el = makeStubEl();
    this.statusBarItems.push(el);
    return el;
  }
  addSettingTab(t: unknown): void {
    this.settingTabs.push(t);
  }
}

export class PluginSettingTab {
  containerEl = makeStubEl();
  constructor(
    public app: App,
    public plugin: Plugin,
  ) {}
  display(): void {}
  hide(): void {}
}

export class Setting {
  constructor(public containerEl: unknown) {}
  setName(_: string): this {
    return this;
  }
  setDesc(_: string): this {
    return this;
  }
  addText(cb: (t: TextComponent) => void): this {
    cb(new TextComponent());
    return this;
  }
  addToggle(cb: (t: ToggleComponent) => void): this {
    cb(new ToggleComponent());
    return this;
  }
  addTextArea(cb: (t: TextComponent) => void): this {
    cb(new TextComponent());
    return this;
  }
  addButton(cb: (b: ButtonComponent) => void): this {
    cb(new ButtonComponent());
    return this;
  }
  addDropdown(cb: (d: DropdownComponent) => void): this {
    cb(new DropdownComponent());
    return this;
  }
}

export class TextComponent {
  inputEl = makeStubEl();
  private value = '';
  setPlaceholder(_: string): this {
    return this;
  }
  setValue(v: string): this {
    this.value = v;
    return this;
  }
  getValue(): string {
    return this.value;
  }
  setDisabled(_: boolean): this {
    return this;
  }
  onChange(_: (v: string) => void): this {
    return this;
  }
}

export class ToggleComponent {
  setValue(_: boolean): this {
    return this;
  }
  setDisabled(_: boolean): this {
    return this;
  }
  onChange(_: (v: boolean) => void): this {
    return this;
  }
}

export class ButtonComponent {
  buttonEl = makeStubEl();
  private clickHandler: (() => void) | null = null;
  setButtonText(_: string): this {
    return this;
  }
  setTooltip(_: string): this {
    return this;
  }
  setCta(): this {
    return this;
  }
  setWarning(): this {
    return this;
  }
  onClick(h: () => void): this {
    this.clickHandler = h;
    return this;
  }
  click(): void {
    this.clickHandler?.();
  }
}

export class DropdownComponent {
  addOption(_v: string, _l: string): this {
    return this;
  }
  setValue(_: string): this {
    return this;
  }
  onChange(_: (v: string) => void): this {
    return this;
  }
}

export class Modal {
  contentEl = makeStubEl();
  constructor(public app: App) {}
  open(): void {
    this.onOpen();
  }
  close(): void {
    this.onClose();
  }
  onOpen(): void {}
  onClose(): void {}
}

export class TAbstractFile {
  constructor(public path: string) {}
  get name(): string {
    const slash = this.path.lastIndexOf('/');
    return slash === -1 ? this.path : this.path.slice(slash + 1);
  }
}
export class TFile extends TAbstractFile {
  get extension(): string {
    const dot = this.name.lastIndexOf('.');
    return dot === -1 ? '' : this.name.slice(dot + 1);
  }
  get basename(): string {
    const dot = this.name.lastIndexOf('.');
    return dot === -1 ? this.name : this.name.slice(0, dot);
  }
}
export class TFolder extends TAbstractFile {
  children: TAbstractFile[] = [];
}

function makeStubEl(): HTMLElementLike {
  const classes = new Set<string>();
  let text = '';
  const handlers = new Map<string, Set<(e: unknown) => void>>();
  return {
    setText(t: string) {
      text = t;
    },
    getText() {
      return text;
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
    createEl(_tag: string, opts?: { text?: string; cls?: string }): HTMLElementLike {
      return makeStubEl();
    },
    addEventListener(name: string, h: (e: unknown) => void) {
      let s = handlers.get(name);
      if (!s) {
        s = new Set();
        handlers.set(name, s);
      }
      s.add(h);
    },
    removeEventListener(name: string, h: (e: unknown) => void) {
      handlers.get(name)?.delete(h);
    },
  };
}

export interface HTMLElementLike {
  setText(t: string): void;
  getText(): string;
  addClass(c: string): void;
  removeClass(c: string): void;
  hasClass(c: string): boolean;
  empty(): void;
  createEl(tag: string, opts?: { text?: string; cls?: string }): HTMLElementLike;
  addEventListener(name: string, h: (e: unknown) => void): void;
  removeEventListener(name: string, h: (e: unknown) => void): void;
}
