import { describe, expect, it } from 'vitest';
import {
  autoRefreshes,
  granularityAllowed,
  intervalValid,
  latencyLevel,
  rowError,
  toPayload,
  validHost,
  type TargetRow,
} from './latency';

const row = (over: Partial<TargetRow> = {}): TargetRow => ({
  key: 'k',
  name: 'Beijing',
  carrier: 'telecom',
  kind: 'tcp',
  host: '192.0.2.1',
  port: 443,
  ...over,
});

describe('validHost (same rule as the server)', () => {
  it('accepts addresses and names', () => {
    for (const h of ['192.0.2.1', '2001:db8::1', '::1', 'example.com', 'a-b.example.com', 'example.com.', '::ffff:192.0.2.1']) {
      expect(validHost(h), h).toBe(true);
    }
  });

  it('refuses everything else', () => {
    for (const h of ['', '999.1.1.1', '1.2.3', 'http://example.com', 'example.com:443', 'a b', '-a.example.com', 'a..b', '[2001:db8::1]', 'example.com/x', '1:2:3', '1::2::3']) {
      expect(validHost(h), h).toBe(false);
    }
    expect(validHost('a'.repeat(64) + '.com')).toBe(false);
  });
});

describe('rowError', () => {
  it('passes a good row, TCP and ICMP', () => {
    expect(rowError(row())).toBeNull();
    expect(rowError(row({ kind: 'icmp', port: null }))).toBeNull();
  });

  it('names the first thing wrong', () => {
    expect(rowError(row({ name: '  ' }))).toBe('latencyErrName');
    expect(rowError(row({ name: '电'.repeat(32) }))).toBeNull();
    expect(rowError(row({ name: '电'.repeat(33) }))).toBe('latencyErrNameLong');
    expect(rowError(row({ host: 'not a host' }))).toBe('latencyErrHost');
    expect(rowError(row({ port: null }))).toBe('latencyErrPort');
    expect(rowError(row({ port: 0 }))).toBe('latencyErrPort');
    expect(rowError(row({ port: 65536 }))).toBe('latencyErrPort');
    expect(rowError(row({ port: 1.5 }))).toBe('latencyErrPort');
  });
});

describe('intervalValid', () => {
  it('is 5 to 60, whole', () => {
    expect(intervalValid(5)).toBe(true);
    expect(intervalValid(60)).toBe(true);
    for (const v of [4, 61, 7.5, null, undefined]) expect(intervalValid(v), String(v)).toBe(false);
  });
});

describe('granularity rules', () => {
  it('7 and 30 days allow only the default and hourly', () => {
    for (const r of ['7d', '30d'] as const) {
      expect(granularityAllowed(r, 'auto')).toBe(true);
      expect(granularityAllowed(r, '1h')).toBe(true);
      expect(granularityAllowed(r, '1m')).toBe(false);
      expect(granularityAllowed(r, '5m')).toBe(false);
    }
  });

  it('shorter ranges allow all of them', () => {
    for (const r of ['1h', '6h', '24h'] as const) {
      for (const g of ['auto', '1m', '5m', '1h'] as const) expect(granularityAllowed(r, g)).toBe(true);
    }
  });

  it('only ranges up to 6 hours refresh themselves', () => {
    expect(['1h', '6h', '24h', '7d', '30d'].map((r) => autoRefreshes(r as never))).toEqual([true, true, false, false, false]);
  });
});

describe('latencyLevel', () => {
  it('colours by latency', () => {
    expect(latencyLevel(20, 0)).toBe('good');
    expect(latencyLevel(80, 0)).toBe('good');
    expect(latencyLevel(81, 0)).toBe('warn');
    expect(latencyLevel(200, 0)).toBe('warn');
    expect(latencyLevel(201, 0)).toBe('bad');
  });

  it('loss can only make it worse', () => {
    expect(latencyLevel(20, 1)).toBe('warn');
    expect(latencyLevel(20, 5)).toBe('bad');
    expect(latencyLevel(150, 1)).toBe('warn');
  });

  it('no reading is grey, total loss is bad', () => {
    expect(latencyLevel(null, null)).toBe('none');
    expect(latencyLevel(undefined, undefined)).toBe('none');
    expect(latencyLevel(null, 100)).toBe('bad');
    expect(latencyLevel(20, 100)).toBe('bad');
  });
});

describe('toPayload', () => {
  it('trims, keeps ids, and drops the port of an ICMP target', () => {
    const p = toPayload(15, [
      row({ id: 3, name: ' A ', host: ' example.com ' }),
      row({ key: 'new-1', kind: 'icmp', port: 443 }),
    ]);
    expect(p).toEqual({
      interval_secs: 15,
      targets: [
        { id: 3, name: 'A', carrier: 'telecom', kind: 'tcp', host: 'example.com', port: 443 },
        { name: 'Beijing', carrier: 'telecom', kind: 'icmp', host: '192.0.2.1', port: null },
      ],
    });
  });
});
