/**
 * Rule export/import helpers — extracted from Rules.tsx so the round-trip is
 * unit-testable without mounting the React page.
 *
 * The export format is intentionally MINIMAL (a "share export"): only
 * `dest[]`, `listen_port`, `name`. Its purpose is quick copy/paste and target
 * migration between groups/installs, NOT a full backup (rate limits, load-
 * balance strategy, protocol, group, etc. are deliberately dropped — the import
 * path re-applies sensible defaults via IMPORT_DEFAULTS in Rules.tsx).
 *
 * The golden property this module guarantees: a rule exported by `buildExportJSON`
 * ALWAYS round-trips back through `validateImportEntry` + `parseDest` into the
 * same enabled targets (host/port/enabled), for IPv4, IPv6, single-target, and
 * multi-target rules. See `rulesIO.test.ts`.
 */
import type { ForwardRule, RuleTargetInput } from '../api/types';

/** Mirror of Rules.tsx's ruleTargets(): unfold a rule's targets, falling back
 *  to the legacy target_addr/target_port pair when the targets[] array is empty. */
export function ruleTargets(rule: ForwardRule): RuleTargetInput[] {
  const targets = rule.targets?.length
    ? rule.targets.map(t => ({ host: t.host, port: t.port, enabled: t.enabled }))
    : [{ host: rule.target_addr, port: rule.target_port, enabled: true }];
  return targets;
}

/** Wrap a host:port as a dest string, bracketing IPv6 hosts (`[addr]:port`). */
function formatDest(host: string, port: number): string {
  const h = host.trim();
  const isV6 = h.includes(':') && !h.startsWith('[');
  return isV6 ? `[${h}]:${port}` : `${h}:${port}`;
}

/** The minimal export entry shape. */
export interface ExportEntry {
  dest: string[];
  listen_port: number;
  name: string;
}

/**
 * Build the compact single-line share-export JSON for a set of rules.
 *
 * - Enabled targets only (disabled ones are dropped — they're not active
 *   forwards).
 * - IPv6 hosts are bracketed so the dest parses back unambiguously.
 * - Always emits a JSON ARRAY (even for a single rule) so the output pastes
 *   straight into the import box (which expects `[{...}]`).
 * - Compact (no pretty-print) so it's the one-line shape shown in the import
 *   hint.
 */
/** A group name made safe for a download's file name: whitespace and the
 *  characters Windows or macOS reject become `-`, trimmed, at most 40
 *  characters. '' when nothing usable is left. */
export function exportFileLabel(name: string): string {
  return name
    .trim()
    .replace(/[\\/:*?"<>|\s]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 40);
}

export function buildExportJSON(rules: ForwardRule[]): string {
  const simplified: ExportEntry[] = rules.map(r => {
    const targets = ruleTargets(r).filter(t => t.enabled);
    const dest = targets.map(t => formatDest(t.host, t.port));
    return { dest, listen_port: r.listen_port, name: r.name };
  });
  return JSON.stringify(simplified);
}

/** The dest regex: `[ipv6]` or a non-colon host, then `:port`. Exported so
 *  parseDest and validateImportEntry share ONE definition. */
const DEST_RE = /^(\[.+?\]|[^:]+):(\d+)$/;

/** Parse a `host:port` / `[ipv6]:port` dest string into {host, port}, or null
 *  when malformed. Strips the brackets from an IPv6 host. */
export function parseDest(d: string): { host: string; port: number } | null {
  const m = d.match(DEST_RE);
  if (!m) return null;
  const host = m[1].replace(/^\[|\]$/g, '');
  const port = parseInt(m[2], 10);
  if (!host || port < 1 || port > 65535) return null;
  return { host, port };
}

/** One target row in the rule form, as the form holds it (a fresh row has an
 *  empty host and no port yet). */
export interface TargetRow {
  host: string;
  port?: number | null;
  enabled?: boolean;
}

/**
 * v1.2.15: merge pasted targets into the rule form's target rows. `text` holds
 * `host:port` / `[ipv6]:port` entries separated by new lines, spaces, commas or
 * semicolons. Rows still blank (a fresh form's first row) give way to the
 * pasted ones; an entry already present — or pasted twice — is skipped.
 * Nothing is merged when any entry is malformed: `invalid` lists them, so the
 * user fixes the paste instead of finding half of it applied.
 */
export function mergeBulkTargets(
  current: TargetRow[],
  text: string,
): { targets: TargetRow[]; added: number; skipped: number; invalid: string[] } {
  const entries = text.split(/[\s,，;；]+/).map((s) => s.trim()).filter(Boolean);
  const invalid: string[] = [];
  const parsed: { host: string; port: number }[] = [];
  for (const e of entries) {
    const d = parseDest(e);
    if (d) parsed.push(d);
    else invalid.push(e);
  }
  if (invalid.length > 0) return { targets: current, added: 0, skipped: 0, invalid };

  const kept = current.filter((r) => (r.host ?? '').trim() !== '' || r.port != null);
  const key = (host: string, port: number) => `${host.toLowerCase()}|${port}`;
  const seen = new Set(kept.filter((r) => r.port != null).map((r) => key(r.host.trim(), Number(r.port))));
  const out = [...kept];
  let added = 0;
  let skipped = 0;
  for (const d of parsed) {
    const k = key(d.host, d.port);
    if (seen.has(k)) { skipped += 1; continue; }
    seen.add(k);
    out.push({ host: d.host, port: d.port, enabled: true });
    added += 1;
  }
  return {
    targets: out.length > 0 ? out : [{ host: '', port: undefined, enabled: true }],
    added,
    skipped,
    invalid,
  };
}

/** The loose entry shape the import box accepts (every field optional, validated
 *  by validateImportEntry before use). */
export interface ImportEntry {
  name?: string;
  listen_port?: number;
  dest?: string[];
}

/**
 * Is `x` a plain, non-null object? Guards against the JSON being a bare
 * primitive / null / array at the entry position (e.g. the user pasted `42` or
 * `"[1,2,3]"`). Arrays are objects in JS, so exclude them explicitly — an entry
 * must be a `{...}` record.
 */
export function isImportEntry(x: unknown): x is Record<string, unknown> {
  return typeof x === 'object' && x !== null && !Array.isArray(x);
}

/**
 * Validate a single import entry. Returns a human-readable error string, or
 * null when the entry is well-formed.
 *
 * The input is `unknown` (straight from `JSON.parse`), so EVERY field is
 * runtime-type-checked before its value is inspected — a malformed paste like
 * `{"name": 123, "listen_port": "80", "dest": "1.2.3.4:80"}` must produce a
 * clean "invalid" verdict, NOT a `.trim() is not a function` crash. (The earlier
 * version assumed the fields were already the right type and crashed on
 * wrong-typed JSON.)
 */
export function validateImportEntry(e: unknown): string | null {
  if (!isImportEntry(e)) return 'entry must be an object';
  // name: required, must be a non-empty string after trim.
  const name = e['name'];
  if (typeof name !== 'string' || name.trim() === '') return 'name is required';
  // listen_port: required, must be an integer in the valid range. A numeric
  // string like "80" is rejected (the export emits a real number; accepting
  // strings would silently let "80abc" through Number() later).
  const port = e['listen_port'];
  if (typeof port !== 'number' || !Number.isFinite(port) || port < 1 || port > 65535)
    return 'listen_port must be 1-65535';
  // dest: required, must be a non-empty array of strings.
  const dest = e['dest'];
  if (!Array.isArray(dest) || dest.length === 0) return 'dest must not be empty';
  for (const d of dest) {
    if (typeof d !== 'string') return `invalid dest format: ${String(d)}`;
    if (!parseDest(d)) return `invalid dest format: ${d}`;
  }
  return null;
}

/**
 * Coerce a validated entry to its typed form. ONLY safe to call after
 * `validateImportEntry(e) === null`; the caller MUST have validated first.
 * Centralises the `as` cast so the consuming code (handleImport) doesn't lie
 * about types in multiple places.
 */
export function asValidatedEntry(e: unknown): { name: string; listen_port: number; dest: string[] } {
  // validateImportEntry already checked the runtime types; re-assert here only
  // to satisfy TS. This never throws for an entry that passed validation.
  const o = e as Record<string, unknown>;
  return {
    name: o['name'] as string,
    listen_port: o['listen_port'] as number,
    dest: o['dest'] as string[],
  };
}
