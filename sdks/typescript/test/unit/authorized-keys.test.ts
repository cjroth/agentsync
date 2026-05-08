import { describe, expect, test } from 'bun:test';
import { Identity, parseAuthorizedKeys, renderAuthorizedKeys } from '../../src/index.js';
import type { AuthorizedPeer } from '../../src/index.js';

describe('authorized_keys', () => {
  test('parses ssh-style lines with labels', () => {
    const id = Identity.generate();
    const body = `${id.pubkey().toSshString()} alice\n# comment\n\n`;
    const peers = parseAuthorizedKeys(body);
    expect(peers).toHaveLength(1);
    expect(peers[0]?.pubkey).toBe(id.pubkey().toSshString());
    expect(peers[0]?.label).toBe('alice');
  });

  test('renderAuthorizedKeys round-trips', () => {
    const id = Identity.generate();
    const peers: AuthorizedPeer[] = [{ pubkey: id.pubkey().toSshString(), label: 'bob' }];
    const rendered = renderAuthorizedKeys(peers);
    expect(rendered.includes('ssh-ed25519 ')).toBe(true);
    const reparsed = parseAuthorizedKeys(rendered);
    expect(reparsed).toEqual(peers);
  });

  test('skips comments and blank lines', () => {
    const peers = parseAuthorizedKeys('# only comments\n\n   # indented\n');
    expect(peers).toEqual([]);
  });
});
