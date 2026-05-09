// Node/Bun WebSocket transport using the `ws` package. Exposes the peer
// TLS cert fingerprint via `channelBinding()` so the handshake transcript
// can bind to the underlying TLS channel.

import { createHash } from 'node:crypto';
import type { TransportAdapter, TransportConn } from '../types.js';

interface WsCtor {
  new (url: string, options: { rejectUnauthorized: boolean }): WsLike;
}

interface WsLike {
  binaryType: string;
  send(data: Uint8Array, cb?: (err?: Error) => void): void;
  close(code?: number, reason?: string): void;
  on(event: 'open', cb: () => void): void;
  on(event: 'message', cb: (data: Buffer | ArrayBuffer | Buffer[]) => void): void;
  on(event: 'close', cb: () => void): void;
  on(event: 'error', cb: (err: Error) => void): void;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  _socket?: { getPeerCertificate?: (detailed: boolean) => any };
}

export function nodeWsTransport(WS: WsCtor): TransportAdapter {
  return {
    async connect(url: string): Promise<TransportConn> {
      const ws = new WS(url, { rejectUnauthorized: false });
      ws.binaryType = 'nodebuffer';
      await new Promise<void>((res, rej) => {
        ws.on('open', () => res());
        ws.on('error', (e: Error) => rej(e));
      });
      const incoming: Uint8Array[] = [];
      let waiter: ((v: Uint8Array | null) => void) | null = null;
      let closed = false;
      ws.on('message', (data: Buffer | ArrayBuffer | Buffer[]) => {
        const bytes = toBytes(data);
        if (waiter) {
          const w = waiter;
          waiter = null;
          w(bytes);
        } else {
          incoming.push(bytes);
        }
      });
      ws.on('close', () => {
        closed = true;
        if (waiter) {
          const w = waiter;
          waiter = null;
          w(null);
        }
      });
      const channelBindingBytes = extractCertFingerprint(ws);
      return {
        async send(bytes: Uint8Array) {
          await new Promise<void>((res, rej) => {
            ws.send(bytes, (err?: Error) => (err ? rej(err) : res()));
          });
        },
        async *recv() {
          while (true) {
            if (incoming.length > 0) {
              yield incoming.shift()!;
              continue;
            }
            if (closed) return;
            const next = await new Promise<Uint8Array | null>((res) => {
              waiter = res;
            });
            if (next === null) return;
            yield next;
          }
        },
        channelBinding(): Uint8Array | null {
          return channelBindingBytes;
        },
        async close() {
          ws.close();
        },
      };
    },
  };
}

function toBytes(data: Buffer | ArrayBuffer | Buffer[]): Uint8Array {
  if (Array.isArray(data)) {
    const total = data.reduce((n, b) => n + b.length, 0);
    const out = new Uint8Array(total);
    let off = 0;
    for (const b of data) {
      out.set(b, off);
      off += b.length;
    }
    return out;
  }
  if (data instanceof ArrayBuffer) return new Uint8Array(data);
  return new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
}

function extractCertFingerprint(ws: WsLike): Uint8Array | null {
  const sock = ws._socket;
  if (!sock?.getPeerCertificate) return null;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const cert: any = sock.getPeerCertificate(true);
  if (!cert?.raw) return null;
  const der: Buffer = cert.raw;
  return new Uint8Array(createHash('sha256').update(der).digest());
}
