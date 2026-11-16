// Latency monitor (v1.2.21): rules shared by the settings section and the
// latency page. The limits mirror crates/panel/src/service/latency.rs — the
// server enforces them for real; this only saves a round trip and says which
// field is wrong.

export const LATENCY_MAX_TARGETS = 16;
export const LATENCY_MIN_INTERVAL = 5;
export const LATENCY_MAX_INTERVAL = 60;
export const LATENCY_DEFAULT_INTERVAL = 10;
export const LATENCY_MAX_NAME = 32;

export const CARRIERS = ['telecom', 'unicom', 'mobile', 'other'] as const;
export type Carrier = (typeof CARRIERS)[number];
export const KINDS = ['tcp', 'icmp'] as const;
export type LatencyKind = (typeof KINDS)[number];

/** Round trip, in ms, up to which a target reads as good / acceptable. Above
 *  the second it reads as poor. One place, so the cards and the legend agree. */
export const LATENCY_GOOD_MS = 80;
export const LATENCY_WARN_MS = 200;
/** Loss, in percent, from which a card is flagged whatever its latency. */
export const LATENCY_LOSS_WARN_PCT = 1;
export const LATENCY_LOSS_BAD_PCT = 5;

export type LatencyLevel = 'good' | 'warn' | 'bad' | 'none';

/** How a card is coloured: by the worse of its latency and its loss. A target
 *  with no reading at all is 'none' (grey), not 'good'. */
export function latencyLevel(avgMs: number | null | undefined, lossPct: number | null | undefined): LatencyLevel {
  if (lossPct != null && lossPct >= 100) return 'bad';
  if (avgMs == null) return lossPct != null && lossPct > 0 ? 'bad' : 'none';
  let level: LatencyLevel = avgMs <= LATENCY_GOOD_MS ? 'good' : avgMs <= LATENCY_WARN_MS ? 'warn' : 'bad';
  if (lossPct != null) {
    if (lossPct >= LATENCY_LOSS_BAD_PCT) level = 'bad';
    else if (lossPct >= LATENCY_LOSS_WARN_PCT && level === 'good') level = 'warn';
  }
  return level;
}

export const LEVEL_COLORS: Record<LatencyLevel, string> = {
  good: '#52c41a',
  warn: '#faad14',
  bad: '#ff4d4f',
  none: '#bfbfbf',
};

// ── Status strips ──

/** How many equal slices the selected range is cut into for the strips. */
export const STRIP_SEGMENTS = 30;
/** A strip slice with loss under this (percent) is amber, from it red. */
export const STRIP_LOSS_BAD_PCT = LATENCY_LOSS_BAD_PCT;

/** Colour class of a loss strip slice: 0% green, above 0 and under 5% amber,
 *  5% and up red, no data grey. */
export function lossStripLevel(lossPct: number | null | undefined): LatencyLevel {
  if (lossPct == null) return 'none';
  if (lossPct <= 0) return 'good';
  return lossPct < STRIP_LOSS_BAD_PCT ? 'warn' : 'bad';
}

// ── Range and granularity ──

export const RANGES = ['1h', '6h', '24h', '7d', '30d'] as const;
export type LatencyRange = (typeof RANGES)[number];
export const GRANULARITIES = ['auto', '1m', '5m', '1h'] as const;
export type LatencyGranularity = (typeof GRANULARITIES)[number];

/** Minute rows are kept 48 hours, so a 7- or 30-day view can only be hourly. */
export function granularityAllowed(range: LatencyRange, g: LatencyGranularity): boolean {
  if (range === '7d' || range === '30d') return g === 'auto' || g === '1h';
  return true;
}

/** Ranges up to 6 hours follow the clock: they are re-read every minute. */
export function autoRefreshes(range: LatencyRange): boolean {
  return range === '1h' || range === '6h' || range === '24h';
}

export const AUTO_REFRESH_MS = 60_000;

// ── Settings validation ──

export interface TargetRow {
  /** Stable key for React and drag-and-drop; the server id for saved rows. */
  key: string;
  /** Present for a saved target (kept, with its history, on save). */
  id?: number;
  name: string;
  carrier: Carrier;
  kind: LatencyKind;
  host: string;
  port: number | null;
}

/** An IP address, or a DNS name of letter-digit-hyphen labels whose last label
 *  has a letter — the same test the server applies. */
export function validHost(host: string): boolean {
  if (!host || host.length > 253) return false;
  if (isIpAddress(host)) return true;
  const name = host.endsWith('.') ? host.slice(0, -1) : host;
  if (!name) return false;
  const labels = name.split('.');
  const okLabel = (l: string) =>
    l.length >= 1 && l.length <= 63 && !l.startsWith('-') && !l.endsWith('-') && /^[A-Za-z0-9-]+$/.test(l);
  return labels.every(okLabel) && /[A-Za-z]/.test(labels[labels.length - 1]);
}

function isIpAddress(s: string): boolean {
  const v4 = /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/.exec(s);
  if (v4) return v4.slice(1).every((p) => Number(p) <= 255);
  // IPv6: hex groups with at most one "::"; an embedded IPv4 tail is allowed.
  if (!/^[0-9A-Fa-f:.]+$/.test(s) || !s.includes(':')) return false;
  const halves = s.split('::');
  if (halves.length > 2) return false;
  const groups = (part: string) => (part === '' ? [] : part.split(':'));
  const all = [...groups(halves[0]), ...(halves.length === 2 ? groups(halves[1]) : [])];
  let count = 0;
  for (let i = 0; i < all.length; i += 1) {
    const g = all[i];
    if (i === all.length - 1 && g.includes('.')) {
      if (!isIpAddress(g)) return false;
      count += 2;
    } else if (/^[0-9A-Fa-f]{1,4}$/.test(g)) {
      count += 1;
    } else {
      return false;
    }
  }
  return halves.length === 2 ? count < 8 : count === 8;
}

/** What is wrong with one row, as the i18n key of the message (or null). */
export type RowError =
  | 'latencyErrName'
  | 'latencyErrNameLong'
  | 'latencyErrHost'
  | 'latencyErrPort'
  | null;

export function rowError(r: TargetRow): RowError {
  const name = r.name.trim();
  if (!name) return 'latencyErrName';
  if ([...name].length > LATENCY_MAX_NAME) return 'latencyErrNameLong';
  if (!validHost(r.host.trim())) return 'latencyErrHost';
  if (r.kind === 'tcp' && !(Number.isInteger(r.port) && (r.port as number) >= 1 && (r.port as number) <= 65535)) {
    return 'latencyErrPort';
  }
  return null;
}

export function intervalValid(v: number | null | undefined): v is number {
  return v != null && Number.isInteger(v) && v >= LATENCY_MIN_INTERVAL && v <= LATENCY_MAX_INTERVAL;
}

/** The body of PUT /admin/groups/{id}/latency/config. */
export function toPayload(interval: number, rows: TargetRow[]) {
  return {
    interval_secs: interval,
    targets: rows.map((r) => ({
      ...(r.id != null ? { id: r.id } : {}),
      name: r.name.trim(),
      carrier: r.carrier,
      kind: r.kind,
      host: r.host.trim(),
      port: r.kind === 'tcp' ? r.port : null,
    })),
  };
}
