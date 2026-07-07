import { describe, expect, it } from 'vitest';
import {
  buildExportJSON,
  parseDest,
  ruleTargets,
  validateImportEntry,
  type ExportEntry,
  type ImportEntry,
} from './rulesIO';
import type { ForwardRule } from '../api/types';

/** Build a ForwardRule fixture with the minimum fields the export/import code
 *  reads (id, name, listen_port, targets, target_addr/port). */
function mkRule(over: Partial<ForwardRule>): ForwardRule {
  return {
    id: 1,
    name: 'r1',
    uid: 1,
    paused: false,
    listen_port: 10000,
    protocol: 'tcp_udp',
    device_group_in: 1,
    device_group_out: null,
    forward_mode: 'direct',
    target_addr: '1.1.1.1',
    target_port: 80,
    ...over,
  } as ForwardRule;
}

/** Build a ForwardRuleTarget fixture (fills the boilerplate created_at). */
function tgt(host: string, port: number, enabled: boolean, id: number, rule_id: number, position: number) {
  return { id, rule_id, host, port, position, enabled, created_at: '' };
}

describe('ruleTargets', () => {
  it('unfolds the targets[] array when present', () => {
    const r = mkRule({
      targets: [
        tgt('a.com', 1, true, 1, 1, 1),
        tgt('b.com', 2, false, 2, 1, 2),
      ],
    });
    expect(ruleTargets(r)).toEqual([
      { host: 'a.com', port: 1, enabled: true },
      { host: 'b.com', port: 2, enabled: false },
    ]);
  });

  it('falls back to the legacy target_addr/target_port when targets[] is empty', () => {
    const r = mkRule({ targets: [], target_addr: 'legacy.host', target_port: 9999 });
    expect(ruleTargets(r)).toEqual([{ host: 'legacy.host', port: 9999, enabled: true }]);
  });
});

describe('buildExportJSON', () => {
  it('emits a compact single-line JSON array', () => {
    const out = buildExportJSON([mkRule({ name: 'r1', listen_port: 10000, target_addr: '1.1.1.1', target_port: 80 })]);
    // Single line, no pretty-print indentation.
    expect(out).not.toContain('\n');
    expect(out.startsWith('[{')).toBe(true);
    expect(out.endsWith('}]')).toBe(true);
  });

  it('always emits an array even for a single rule (round-trips into the import box)', () => {
    const out = buildExportJSON([mkRule({})]);
    const parsed = JSON.parse(out);
    expect(Array.isArray(parsed)).toBe(true);
    expect(parsed).toHaveLength(1);
  });

  it('drops disabled targets and keeps enabled ones', () => {
    const r = mkRule({
      targets: [
        tgt('keep.com', 1, true, 1, 1, 1),
        tgt('drop.com', 2, false, 2, 1, 2),
        tgt('keep2.com', 3, true, 3, 1, 3),
      ],
    });
    const entry: ExportEntry = JSON.parse(buildExportJSON([r]))[0];
    expect(entry.dest).toEqual(['keep.com:1', 'keep2.com:3']);
  });

  it('wraps IPv6 hosts in brackets, leaves IPv4 bare', () => {
    const r = mkRule({
      targets: [
        tgt('2001:db8::1', 443, true, 1, 1, 1),
        tgt('93.184.216.34', 80, true, 2, 1, 2),
      ],
    });
    const entry: ExportEntry = JSON.parse(buildExportJSON([r]))[0];
    expect(entry.dest).toEqual(['[2001:db8::1]:443', '93.184.216.34:80']);
  });

  it('only carries dest / listen_port / name (the minimal share shape)', () => {
    const out = JSON.parse(buildExportJSON([mkRule({ name: 'r1', listen_port: 10000, target_addr: '1.1.1.1', target_port: 80 })]));
    expect(Object.keys(out[0]).sort()).toEqual(['dest', 'listen_port', 'name']);
  });
});

describe('parseDest', () => {
  it('parses IPv4 host:port', () => {
    expect(parseDest('1.2.3.4:80')).toEqual({ host: '1.2.3.4', port: 80 });
  });
  it('parses a bracketed IPv6 host', () => {
    expect(parseDest('[2001:db8::1]:443')).toEqual({ host: '2001:db8::1', port: 443 });
  });
  it('parses a hostname', () => {
    expect(parseDest('example.com:8080')).toEqual({ host: 'example.com', port: 8080 });
  });
  it('returns null for a bare IPv6 (no brackets, ambiguous colons)', () => {
    expect(parseDest('2001:db8::1:443')).toBeNull();
  });
  it('returns null for out-of-range / missing port', () => {
    expect(parseDest('host:0')).toBeNull();
    expect(parseDest('host:65536')).toBeNull();
    expect(parseDest('host:')).toBeNull();
    expect(parseDest(':80')).toBeNull();
  });
});

describe('validateImportEntry', () => {
  it('accepts a well-formed entry', () => {
    expect(validateImportEntry({ name: 'r1', listen_port: 10000, dest: ['1.1.1.1:80'] })).toBeNull();
  });
  it('rejects an empty/missing name', () => {
    expect(validateImportEntry({ name: '', listen_port: 10000, dest: ['1.1.1.1:80'] })).toMatch(/name/);
    expect(validateImportEntry({ listen_port: 10000, dest: ['1.1.1.1:80'] })).toMatch(/name/);
  });
  it('rejects an out-of-range listen_port', () => {
    expect(validateImportEntry({ name: 'r1', listen_port: 0, dest: ['1.1.1.1:80'] })).toMatch(/listen_port/);
    expect(validateImportEntry({ name: 'r1', listen_port: 70000, dest: ['1.1.1.1:80'] })).toMatch(/listen_port/);
  });
  it('rejects an empty dest array', () => {
    expect(validateImportEntry({ name: 'r1', listen_port: 10000, dest: [] })).toMatch(/dest/);
  });
  it('rejects a malformed dest', () => {
    expect(validateImportEntry({ name: 'r1', listen_port: 10000, dest: ['no-port'] })).toMatch(/dest format/);
    expect(validateImportEntry({ name: 'r1', listen_port: 10000, dest: ['host:0'] })).toMatch(/dest format/);
  });
});

// ── THE golden round-trip: an export from buildExportJSON must always re-import
// cleanly (every entry validates, and the parsed targets match the original
// enabled targets exactly). This is the property PR3 pins.
describe('export → import round-trip', () => {
  const cases: Array<{ name: string; rule: ForwardRule; expectedTargets: { host: string; port: number }[] }> = [
    {
      name: 'single IPv4 target',
      rule: mkRule({ name: 'a', listen_port: 10000, target_addr: '1.1.1.1', target_port: 80 }),
      expectedTargets: [{ host: '1.1.1.1', port: 80 }],
    },
    {
      name: 'multiple IPv4 targets',
      rule: mkRule({
        name: 'b',
        listen_port: 10001,
        targets: [
          tgt('a.com', 1, true, 1, 1, 1),
          tgt('b.com', 2, true, 2, 1, 2),
          tgt('c.com', 3, true, 3, 1, 3),
        ],
      }),
      expectedTargets: [{ host: 'a.com', port: 1 }, { host: 'b.com', port: 2 }, { host: 'c.com', port: 3 }],
    },
    {
      name: 'IPv6 target (bracketed on export, unbracketed on import)',
      rule: mkRule({
        name: 'c',
        listen_port: 10002,
        targets: [tgt('2001:db8::1', 443, true, 1, 1, 1)],
      }),
      expectedTargets: [{ host: '2001:db8::1', port: 443 }],
    },
    {
      name: 'mixed IPv4 + IPv6 targets',
      rule: mkRule({
        name: 'd',
        listen_port: 10003,
        targets: [
          tgt('93.184.216.34', 80, true, 1, 1, 1),
          tgt('2001:db8::9', 443, true, 2, 1, 2),
        ],
      }),
      expectedTargets: [{ host: '93.184.216.34', port: 80 }, { host: '2001:db8::9', port: 443 }],
    },
    {
      name: 'disabled targets are dropped (only enabled round-trips)',
      rule: mkRule({
        name: 'e',
        listen_port: 10004,
        targets: [
          tgt('on.com', 1, true, 1, 1, 1),
          tgt('off.com', 2, false, 2, 1, 2),
        ],
      }),
      expectedTargets: [{ host: 'on.com', port: 1 }],
    },
    {
      name: 'host with surrounding spaces is trimmed',
      rule: mkRule({
        name: 'f',
        listen_port: 10005,
        targets: [tgt('  spaced.com  ', 8080, true, 1, 1, 1)],
      }),
      expectedTargets: [{ host: 'spaced.com', port: 8080 }],
    },
  ];

  for (const { name, rule, expectedTargets } of cases) {
    it(`round-trips: ${name}`, () => {
      // 1. Export the rule.
      const exported = buildExportJSON([rule]);
      // 2. Parse the export back into entries (what the import box does).
      const entries = JSON.parse(exported) as ImportEntry[];
      expect(entries).toHaveLength(1);
      // 3. EVERY exported entry must pass import validation.
      const err = validateImportEntry(entries[0]);
      expect(err, `exported entry failed validation: ${err}`).toBeNull();
      // 4. The parsed dest targets must match the original ENABLED targets
      //    (host + port), order preserved.
      const parsed = (entries[0].dest ?? []).map(d => parseDest(d));
      expect(parsed.every(p => p !== null)).toBe(true);
      expect(parsed.map(p => (p as { host: string; port: number })).map(p => ({ host: p.host, port: p.port })))
        .toEqual(expectedTargets);
      // 5. The name + listen_port survive intact.
      expect(entries[0].name).toBe(rule.name);
      expect(entries[0].listen_port).toBe(rule.listen_port);
    });
  }

  it('round-trips a mixed set of rules in one export array', () => {
    const rules = [
      mkRule({ id: 1, name: 'first', listen_port: 10000, target_addr: '1.1.1.1', target_port: 80 }),
      mkRule({
        id: 2, name: 'second', listen_port: 10001,
        targets: [
          tgt('a.com', 1, true, 1, 2, 1),
          tgt('b.com', 2, true, 2, 2, 2),
        ],
      }),
      mkRule({
        id: 3, name: 'third', listen_port: 10002,
        targets: [tgt('2001:db8::1', 443, true, 1, 3, 1)],
      }),
    ];
    const exported = buildExportJSON(rules);
    const entries = JSON.parse(exported) as ImportEntry[];
    expect(entries).toHaveLength(3);
    // Every entry validates.
    for (const e of entries) {
      expect(validateImportEntry(e), `entry ${e.name} failed`).toBeNull();
    }
    // Names + ports survived in order.
    expect(entries.map(e => e.name)).toEqual(['first', 'second', 'third']);
    expect(entries.map(e => e.listen_port)).toEqual([10000, 10001, 10002]);
    // The second rule's two targets round-trip into two dests.
    expect(entries[1].dest).toEqual(['a.com:1', 'b.com:2']);
  });
});
