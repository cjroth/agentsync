// Codec for the on-disk identity file the native `agentsync` CLI uses,
// so the TS SDK / Obsidian plugin can share `~/.agentsync/id_ed25519`
// byte-for-byte with the CLI (same device, same key).
//
// Format mirrors `crates/agentsync-core/src/identity.rs`:
//
//   <path>      one line: `agentsync-identity-v1 <base64nopad(32-byte seed)>\n`
//   <path>.pub  one line: `<ssh-ed25519 wire-format pubkey>\n`
//
// base64 is the STANDARD alphabet with **no padding** (Rust's
// `STANDARD_NO_PAD`); decoding tolerates a stray `=` just in case.

const PREFIX = 'agentsync-identity-v1 ';
const SEED_LEN = 32;
const B64 = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';

function b64encodeNoPad(bytes: Uint8Array): string {
  let out = '';
  for (let i = 0; i < bytes.length; i += 3) {
    const b0 = bytes[i] as number;
    const b1 = i + 1 < bytes.length ? (bytes[i + 1] as number) : 0;
    const b2 = i + 2 < bytes.length ? (bytes[i + 2] as number) : 0;
    const n = (b0 << 16) | (b1 << 8) | b2;
    const chunk = bytes.length - i;
    out += B64.charAt((n >> 18) & 63) + B64.charAt((n >> 12) & 63);
    if (chunk > 1) out += B64.charAt((n >> 6) & 63);
    if (chunk > 2) out += B64.charAt(n & 63);
  }
  return out;
}

function b64decode(s: string): Uint8Array {
  const clean = s.replace(/=+$/, '');
  const out: number[] = [];
  let acc = 0;
  let bits = 0;
  for (const ch of clean) {
    const v = B64.indexOf(ch);
    if (v === -1) throw new Error(`invalid base64 character: ${JSON.stringify(ch)}`);
    acc = (acc << 6) | v;
    bits += 6;
    if (bits >= 8) {
      bits -= 8;
      out.push((acc >> bits) & 0xff);
    }
  }
  return Uint8Array.from(out);
}

/** Serialize a 32-byte ed25519 seed to the `agentsync-identity-v1` line
 * (trailing newline included, matching the Rust writer). */
export function formatAgentsyncIdentity(seed: Uint8Array): string {
  if (seed.length !== SEED_LEN) {
    throw new Error(`identity seed wrong length: got ${seed.length}, want ${SEED_LEN}`);
  }
  return `${PREFIX}${b64encodeNoPad(seed)}\n`;
}

/** Parse an `agentsync-identity-v1` file body, returning the 32-byte seed.
 * Reads only the first line so a `.pub`-style trailer is harmless. */
export function parseAgentsyncIdentity(text: string): Uint8Array {
  const line = text.split('\n')[0] ?? '';
  if (!line.startsWith(PREFIX)) {
    throw new Error('identity file is not in agentsync-identity-v1 format');
  }
  const seed = b64decode(line.slice(PREFIX.length).trim());
  if (seed.length !== SEED_LEN) {
    throw new Error(`identity seed wrong length: got ${seed.length}, want ${SEED_LEN}`);
  }
  return seed;
}

/** Content for the `<path>.pub` sidecar: the SSH wire-format pubkey plus a
 * trailing newline, exactly as the CLI writes it. */
export function formatPubkeySidecar(sshPubkey: string): string {
  return `${sshPubkey}\n`;
}
