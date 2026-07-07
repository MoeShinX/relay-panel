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

/** The loose entry shape the import box accepts (every field optional, validated
 *  by validateImportEntry before use). */
export interface ImportEntry {
  name?: string;
  listen_port?: number;
  dest?: string[];
}

/** Validate a single import entry. Returns a human-readable error string, or
 *  null when the entry is well-formed. Mirrors the original Rules.tsx logic. */
export function validateImportEntry(e: ImportEntry): string | null {
  if (!e.name || e.name.trim() === '') return 'name is required';
  if (e.listen_port == null || e.listen_port < 1 || e.listen_port > 65535)
    return 'listen_port must be 1-65535';
  if (!e.dest || e.dest.length === 0) return 'dest must not be empty';
  for (const d of e.dest) {
    if (!parseDest(d)) return `invalid dest format: ${d}`;
  }
  return null;
}
