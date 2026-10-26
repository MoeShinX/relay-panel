import { describe, expect, it } from 'vitest';
import { isConnectHost, isHostname, isIpAddress, joinHosts, splitHosts } from './connectHosts';

describe('isIpAddress', () => {
  it.each([
    '1.2.3.4', '0.0.0.0', '255.255.255.255', '69.165.73.45',
    '::', '::1', '2001:db8::1', '240e:96c:7100:1fe:1c8:c8aa:4ecc:5d',
    '1:2:3:4:5:6:7:8', '::ffff:1.2.3.4', 'fe80::1', '1::',
  ])('accepts %s', (s) => expect(isIpAddress(s)).toBe(true));

  it.each([
    '', 'node.example.com', '1.2.3', '1.2.3.4.5', '256.1.1.1', '01.2.3.4', '1.2.3.4:80',
    '[::1]', ':::', '1:2:3:4:5:6:7', '1:2:3:4:5:6:7:8:9', '1::2::3', ':1:2:3:4:5:6:7',
    '12345::1', 'g::1', '1.2.3.4/24', 'fe80::1%eth0', '::ffff:1.2.3', ' 1.2.3.4',
  ])('rejects %j', (s) => expect(isIpAddress(s)).toBe(false));
});

describe('splitHosts / joinHosts', () => {
  it('reads commas, spaces and semicolons, and ignores empties', () => {
    expect(splitHosts('1.1.1.1, 2.2.2.2  3.3.3.3;4.4.4.4,')).toEqual(['1.1.1.1', '2.2.2.2', '3.3.3.3', '4.4.4.4']);
    expect(splitHosts('')).toEqual([]);
    expect(splitHosts(undefined)).toEqual([]);
  });
  it('stores trimmed, de-duplicated, comma-separated, and round-trips', () => {
    const stored = joinHosts([' 1.1.1.1 ', '', '2001:db8::1', '1.1.1.1', undefined]);
    expect(stored).toBe('1.1.1.1,2001:db8::1');
    expect(splitHosts(stored)).toEqual(['1.1.1.1', '2001:db8::1']);
  });
  it('leaves a single legacy value (even a hostname) as it was', () => {
    expect(joinHosts(splitHosts('node.example.com'))).toBe('node.example.com');
  });
});

describe('isHostname / isConnectHost', () => {
  it.each([
    'node.example.com', 'a.b.c.d.example.org', 'node1', 'localhost', 'my-node.example.com',
    'xn--fiqs8s.example', 'EXAMPLE.com', 'a'.repeat(63) + '.com', '1node.example.com',
  ])('accepts the name %s', (s) => {
    expect(isHostname(s)).toBe(true);
    expect(isConnectHost(s)).toBe(true);
  });

  it.each([
    '', ' node.example.com', 'node.example.com ', 'node_1.example.com', '-node.example.com',
    'node-.example.com', 'node..example.com', '.example.com', 'node.example.com.',
    'node.example.com:8080', 'http://node.example.com', '节点.example.com',
    'a'.repeat(64) + '.com', ('a.'.repeat(130)) + 'com',
    // Look like addresses, are not: refused rather than taken for a name.
    '256.1.1.1', '1.2.3', '1.2.3.4.5', '123',
  ])('rejects %j', (s) => {
    expect(isConnectHost(s)).toBe(false);
  });

  it('still takes addresses', () => {
    for (const s of ['1.2.3.4', '2001:db8::1']) expect(isConnectHost(s)).toBe(true);
  });
});
