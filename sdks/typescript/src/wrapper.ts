// Re-exports the wasm-bindgen surface as a typed module. The bundler and
// nodejs glue files are byte-for-byte identical at the JS API level —
// this wrapper exists so we have one place to typecheck the API and add
// thin convenience layers later if needed.

import type { AuthorizedPeer, DirectoryMeta, FileMeta, Frame, Label } from './types.js';

// The wasm-pack output declares these as concrete classes. We restate the
// minimum slice we need so consumers don't have to depend on whichever
// .d.ts (bundler vs nodejs) gets picked.
interface WasmModule {
  Identity: typeof IdentityClass;
  Pubkey: typeof PubkeyClass;
  Doc: typeof DocClass;
  SyncState: typeof SyncStateClass;
  parseAuthorizedKeys(body: string): AuthorizedPeer[];
  renderAuthorizedKeys(entries: AuthorizedPeer[]): string;
  randomNonce(): Uint8Array;
  buildTranscript(
    hubNonce: Uint8Array,
    peerNonce: Uint8Array,
    tlsCertFingerprint: Uint8Array,
    hubPubkey: Uint8Array,
    peerPubkey: Uint8Array,
  ): Uint8Array;
  encodeFrame(value: Frame): Uint8Array;
  decodeFrame(bytes: Uint8Array): Frame;
  contentHash(bytes: Uint8Array): string;
  schemaVersion(): number;
  defaultPort(): number;
  normalizeRendezvousUrl(url: string): string;
}

declare class IdentityClass {
  static generate(): IdentityClass;
  static fromSeed(seed: Uint8Array): IdentityClass;
  seed(): Uint8Array;
  pubkey(): PubkeyClass;
  sign(message: Uint8Array): Promise<Uint8Array>;
  free(): void;
}

declare class PubkeyClass {
  static fromBytes(bytes: Uint8Array): PubkeyClass;
  static fromSshString(s: string): PubkeyClass;
  toSshString(): string;
  fingerprint(): string;
  bytes(): Uint8Array;
  verify(message: Uint8Array, signature: Uint8Array): boolean;
  free(): void;
}

declare class SyncStateClass {
  constructor();
  static decode(bytes: Uint8Array): SyncStateClass;
  encode(): Uint8Array;
  free(): void;
}

declare class DocClass {
  constructor(vaultId: string);
  static load(bytes: Uint8Array): DocClass;
  save(): Uint8Array;
  saveIncremental(): Uint8Array;
  vaultId(): string;
  heads(): Uint8Array[];
  merge(other: DocClass): boolean;
  generateSyncMessage(state: SyncStateClass): Uint8Array | undefined;
  receiveSyncMessage(state: SyncStateClass, bytes: Uint8Array): boolean;
  writeTextFile(path: string, content: string): string;
  readFile(path: string): string;
  fileExists(path: string): boolean;
  deleteFile(path: string): void;
  renameFile(from: string, to: string): void;
  writeAttachment(path: string, hash: string, size: number): string;
  listFiles(): FileMeta[];
  createDirectory(path: string): string;
  deleteDirectory(path: string, recursive: boolean): void;
  listDirectories(): DirectoryMeta[];
  createLabel(name: string): void;
  deleteLabel(name: string): void;
  listLabels(): Label[];
  restoreToLabel(name: string): void;
  restoreToTime(targetMs: number): void;
  free(): void;
}

export type Identity = IdentityClass;
export type Pubkey = PubkeyClass;
export type Doc = DocClass;
export type SyncState = SyncStateClass;

export function wrap(mod: unknown): WasmModule {
  // The wasm-pack glue is structurally a WasmModule; cast and re-export.
  // We intentionally do not deep-clone or proxy — the wasm-bindgen objects
  // hold pointers into linear memory and must be `.free()`d when done.
  return mod as WasmModule;
}
