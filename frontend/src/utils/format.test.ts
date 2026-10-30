import { describe, expect, it } from 'vitest';
import { formatBytes, formatBps, formatDateTime, formatPercent, formatUptime } from './format';

const L = { d: 'd', h: 'h', m: 'm', s: 's' };

describe('formatUptime', () => {
  it('returns "-" for missing/invalid input (never 0)', () => {
    expect(formatUptime(undefined, L)).toBe('-');
    expect(formatUptime(null, L)).toBe('-');
    expect(formatUptime(-5, L)).toBe('-');
    expect(formatUptime(NaN, L)).toBe('-');
  });

  it('shows days (+hours) for multi-day uptime', () => {
    // 18 days exactly
    expect(formatUptime(18 * 86400, L)).toBe('18d');
    // 18 days 5 hours
    expect(formatUptime(18 * 86400 + 5 * 3600, L)).toBe('18d 5h');
  });

  it('shows hours (+minutes) below a day', () => {
    expect(formatUptime(10 * 3600, L)).toBe('10h');
    expect(formatUptime(10 * 3600 + 30 * 60, L)).toBe('10h 30m');
  });

  it('shows minutes, then seconds, for small values', () => {
    expect(formatUptime(5 * 60, L)).toBe('5m');
    expect(formatUptime(42, L)).toBe('42s');
    expect(formatUptime(0, L)).toBe('0s');
  });

  it('uses the provided locale labels', () => {
    const zh = { d: '天', h: '小时', m: '分', s: '秒' };
    expect(formatUptime(18 * 86400 + 5 * 3600, zh)).toBe('18天 5小时');
  });
});

describe('formatPercent', () => {
  it('returns "-" for missing input (never 0%)', () => {
    expect(formatPercent(undefined)).toBe('-');
    expect(formatPercent(null)).toBe('-');
  });
  it('formats a percent with one decimal', () => {
    expect(formatPercent(41.2)).toBe('41.2%');
    expect(formatPercent(0)).toBe('0.0%');
  });
});

describe('formatBytes / formatBps', () => {
  it('return "-" for missing input', () => {
    expect(formatBytes(undefined)).toBe('-');
    expect(formatBps(null)).toBe('-');
  });
});

describe('formatDateTime', () => {
  it('turns a nanosecond RFC 3339 stamp into local seconds', () => {
    const out = formatDateTime('2026-10-26T13:11:17.452529009+00:00');
    expect(out).toMatch(/^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}$/);
    // Same instant as the plain UTC stamp, whatever the viewer's timezone.
    expect(formatDateTime('2026-10-26T13:11:17Z')).toBe(out);
  });
  it('shows a dash for nothing and leaves an unparseable value as it came', () => {
    expect(formatDateTime(undefined)).toBe('-');
    expect(formatDateTime(null)).toBe('-');
    expect(formatDateTime('')).toBe('-');
    expect(formatDateTime('not a time')).toBe('not a time');
  });
});
