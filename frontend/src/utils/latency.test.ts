import { describe, expect, it } from 'vitest';
import {
  autoRefreshes,
  granularityAllowed,
  LATENCY_STALE_SECS,
  intervalValid,
  isStaleSample,
  latencyLevel,
  lossStripLevel,
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

describe('latencyLevel with the slow-handshake share', () => {
  it('the 20 ms / 90 % slow example is not green', () => {
    expect(latencyLevel(20, 0, 90)).toBe('bad');
    expect(latencyLevel(20, 0, 0)).toBe('good');
  });

  it('warns from 2 % and is bad from 10 %, like loss does at 1 % and 5 %', () => {
    expect(latencyLevel(20, 0, 1.99)).toBe('good');
    expect(latencyLevel(20, 0, 2)).toBe('warn');
    expect(latencyLevel(20, 0, 9.99)).toBe('warn');
    expect(latencyLevel(20, 0, 10)).toBe('bad');
    // a slow line stays at least as bad as its latency says
    expect(latencyLevel(300, 0, 2)).toBe('bad');
    expect(latencyLevel(150, 0, 0)).toBe('warn');
  });

  it('an unknown share is ignored, never read as 0 % or as a problem', () => {
    expect(latencyLevel(20, 0, null)).toBe('good');
    expect(latencyLevel(20, 0, undefined)).toBe('good');
    expect(latencyLevel(150, 0, null)).toBe('warn');
    expect(latencyLevel(null, null, null)).toBe('none');
  });
});

describe('isStaleSample', () => {
  it('is older than ten minutes, and no sample at all is not stale', () => {
    expect(LATENCY_STALE_SECS).toBe(600);
    expect(isStaleSample(1000, 1600)).toBe(false);
    expect(isStaleSample(1000, 1601)).toBe(true);
    expect(isStaleSample(null, 99999)).toBe(false);
    expect(isStaleSample(undefined, 99999)).toBe(false);
    // a sample from slightly in the future (clock skew) is fresh
    expect(isStaleSample(1700, 1600)).toBe(false);
  });
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

  it('only ranges up to a day refresh themselves', () => {
    expect(['1h', '6h', '24h', '7d', '30d'].map((r) => autoRefreshes(r as never))).toEqual([true, true, true, false, false]);
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

describe('lossStripLevel', () => {
  it('0% green, under 5% amber, 5% and up red, no data grey', () => {
    expect(lossStripLevel(0)).toBe('good');
    expect(lossStripLevel(0.01)).toBe('warn');
    expect(lossStripLevel(4.99)).toBe('warn');
    expect(lossStripLevel(5)).toBe('bad');
    expect(lossStripLevel(100)).toBe('bad');
    expect(lossStripLevel(null)).toBe('none');
    expect(lossStripLevel(undefined)).toBe('none');
  });
});
