import { describe, expect, test } from 'bun:test';
import {
  Identity,
  buildTranscript,
  decodeFrame,
  encodeFrame,
  randomNonce,
} from '../../src/index.js';
import type { Frame } from '../../src/index.js';

describe('Frame codec', () => {
  test('encode → decode round-trips a sync frame', () => {
    const frame: Frame = { t: 'sync', bytes: new Uint8Array([1, 2, 3, 4, 5]) };
    const wire = encodeFrame(frame);
    const decoded = decodeFrame(wire);
    expect(decoded.t).toBe('sync');
    if (decoded.t === 'sync') {
      expect(Array.from(decoded.bytes)).toEqual([1, 2, 3, 4, 5]);
    }
  });

  test('round-trips a hello_hub frame', () => {
    const id = Identity.generate();
    const frame: Frame = {
      t: 'hello_hub',
      vault_id: 'vault-x',
      hub_identity_pubkey: id.pubkey().bytes(),
      hub_nonce: randomNonce(),
      tls_cert_fingerprint: new Uint8Array(32),
      vault_name: 'demo',
    };
    const decoded = decodeFrame(encodeFrame(frame));
    expect(decoded.t).toBe('hello_hub');
    if (decoded.t === 'hello_hub') {
      expect(decoded.vault_id).toBe('vault-x');
      expect(decoded.vault_name).toBe('demo');
    }
  });

  test('rejects garbage bytes', () => {
    expect(() => decodeFrame(new Uint8Array([0xff, 0xff, 0xff]))).toThrow();
  });
});

describe('handshake helpers', () => {
  test('buildTranscript is deterministic for fixed inputs', () => {
    const hubNonce = new Uint8Array(32).fill(1);
    const peerNonce = new Uint8Array(32).fill(2);
    const fp = new Uint8Array(32).fill(3);
    const hubPk = new Uint8Array(32).fill(4);
    const peerPk = new Uint8Array(32).fill(5);
    const a = buildTranscript(hubNonce, peerNonce, fp, hubPk, peerPk);
    const b = buildTranscript(hubNonce, peerNonce, fp, hubPk, peerPk);
    expect(a).toEqual(b);
    // First 17 bytes are the literal "agentsync-auth-v1" domain tag.
    expect(new TextDecoder().decode(a.slice(0, 17))).toBe('agentsync-auth-v1');
  });

  test('buildTranscript rejects wrong-length inputs', () => {
    expect(() =>
      buildTranscript(
        new Uint8Array(31),
        new Uint8Array(32),
        new Uint8Array(0),
        new Uint8Array(32),
        new Uint8Array(32),
      ),
    ).toThrow();
  });
});
