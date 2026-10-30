// Shared formatting helpers for RelayPanel frontend.
//
// Previously `formatBytes` was duplicated in Rules.tsx and Users.tsx; this is
// the single source. Also provides rate/percent formatting for the node-status
// table (disk, network bps, cumulative bytes).
//
// Every helper tolerates null/undefined/NaN and returns "-" so the UI never
// shows NaN/undefined/null (per the node-status enhancement requirements).

const UNITS_BYTES = ['B', 'KB', 'MB', 'GB', 'TB', 'PB'] as const;
const UNITS_BPS = ['B/s', 'KB/s', 'MB/s', 'GB/s', 'TB/s'] as const;

/** Format a byte count into a human-readable string (B / KB / MB / GB / TB). */
export function formatBytes(b?: number | null): string {
  const n = toNum(b);
  if (n === null) return '-';
  return pickUnit(n, UNITS_BYTES, 1024);
}

/** Format a bytes-per-second rate into B/s / KB/s / MB/s / GB/s / TB/s. */
export function formatBps(b?: number | null): string {
  const n = toNum(b);
  if (n === null) return '-';
  return pickUnit(n, UNITS_BPS, 1024);
}

/** Format a 0-100 percentage with one decimal, e.g. 27.3%. Returns "-" if not
 *  a finite number. */
export function formatPercent(p?: number | null): string {
  const n = toNum(p);
  if (n === null) return '-';
  return `${n.toFixed(1)}%`;
}

/** v0.4.14: format an uptime (in SECONDS) into a compact "Nd Nh" / "Nh Nm" /
 *  "Nm" / "Ns" string. Unit labels are passed in (from i18n) so the util stays
 *  locale-agnostic — callers supply localized {d,h,m,s}. Missing/invalid → "-".
 *  Shows the two most significant non-zero units (e.g. 18d 5h), or just the
 *  largest when the rest is zero. */
export function formatUptime(
  secs?: number | null,
  labels: { d: string; h: string; m: string; s: string } = { d: 'd', h: 'h', m: 'm', s: 's' },
): string {
  const n = toNum(secs);
  if (n === null) return '-';
  const total = Math.floor(n);
  const days = Math.floor(total / 86400);
  const hours = Math.floor((total % 86400) / 3600);
  const mins = Math.floor((total % 3600) / 60);
  const s = total % 60;
  if (days > 0) return hours > 0 ? `${days}${labels.d} ${hours}${labels.h}` : `${days}${labels.d}`;
  if (hours > 0) return mins > 0 ? `${hours}${labels.h} ${mins}${labels.m}` : `${hours}${labels.h}`;
  if (mins > 0) return `${mins}${labels.m}`;
  return `${s}${labels.s}`;
}

/** Coerce anything into a finite non-negative number, or null. Used by the
 *  helpers above so the table never renders NaN/undefined. */
function toNum(v: unknown): number | null {
  if (v === null || v === undefined) return null;
  const n = typeof v === 'number' ? v : Number(v);
  if (!Number.isFinite(n) || n < 0) return null;
  return n;
}

/** Walk the unit array, dividing by base until the value fits the next unit. */
function pickUnit(
  value: number,
  units: readonly string[],
  base: number,
): string {
  let v = value;
  let i = 0;
  while (v >= base && i < units.length - 1) {
    v /= base;
    i += 1;
  }
  // Bytes/Bps stay integer; larger units show 2 decimals.
  return i === 0 ? `${v} ${units[i]}` : `${v.toFixed(2)} ${units[i]}`;
}

/** A timestamp from the panel (RFC 3339, often with nanoseconds and a "+00:00"
 *  offset) as the viewer's local "YYYY-MM-DD HH:mm:ss". An unparseable value is
 *  returned as it came rather than hidden; empty gives "-". */
export function formatDateTime(s?: string | null): string {
  if (!s) return '-';
  const d = new Date(s);
  if (Number.isNaN(d.getTime())) return s;
  const p = (n: number) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
}
