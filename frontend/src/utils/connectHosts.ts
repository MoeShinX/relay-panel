/**
 * A device group's "connect address" is stored as ONE string column
 * (`connect_host`), but a group with several nodes has several addresses. They
 * are kept in that same column, comma-separated, so nothing needs migrating and
 * a group with a single address looks exactly as before.
 *
 * The address is only displayed (the rules page's "listen IP"); it never
 * changes what a node listens on.
 */

/** Split a stored/typed value into its addresses (commas, semicolons, spaces). */
export function splitHosts(value: string | null | undefined): string[] {
  return (value ?? '').split(/[\s,;，]+/).filter(Boolean);
}

/** Join addresses for storage: trimmed, empties and repeats dropped. */
export function joinHosts(hosts: Array<string | null | undefined>): string {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const h of hosts) {
    const v = (h ?? '').trim();
    if (v && !seen.has(v)) { seen.add(v); out.push(v); }
  }
  return out.join(',');
}

export function isIPv4(s: string): boolean {
  const parts = s.split('.');
  return parts.length === 4 && parts.every(p =>
    /^\d{1,3}$/.test(p) && Number(p) <= 255 && (p.length === 1 || !p.startsWith('0')));
}

export function isIPv6(s: string): boolean {
  if (!s.includes(':')) return false;
  let str = s;
  if (str.includes('.')) {
    // An embedded IPv4 tail (::ffff:1.2.3.4) stands for the last two groups.
    const i = str.lastIndexOf(':');
    if (!isIPv4(str.slice(i + 1))) return false;
    str = `${str.slice(0, i)}:0:0`;
  }
  const halves = str.split('::');
  if (halves.length > 2) return false;
  const groups = (x: string) => (x === '' ? [] : x.split(':'));
  const head = groups(halves[0]);
  const tail = halves.length === 2 ? groups(halves[1]) : [];
  if (![...head, ...tail].every(g => /^[0-9a-fA-F]{1,4}$/.test(g))) return false;
  const n = head.length + tail.length;
  return halves.length === 2 ? n <= 7 : n === 8;
}

/** A standard IPv4 or IPv6 literal — no brackets, ports or zones. */
export function isIpAddress(s: string): boolean {
  return isIPv4(s) || isIPv6(s);
}

/**
 * A DNS name (RFC 1123): labels of letters, digits and inner hyphens, 1-63
 * characters each, 253 in all. The last label may not be all digits, so a
 * mistyped address such as `256.1.1.1` or `1.2.3` is refused instead of being
 * taken for a name. Internationalised names are written in their `xn--` form.
 */
export function isHostname(s: string): boolean {
  if (s.length === 0 || s.length > 253) return false;
  const labels = s.split('.');
  if (!labels.every(l => /^[A-Za-z0-9]([A-Za-z0-9-]{0,61}[A-Za-z0-9])?$/.test(l))) return false;
  return !/^\d+$/.test(labels[labels.length - 1]);
}

/** What a group's connect address may be: an IP address or a host name. */
export function isConnectHost(s: string): boolean {
  return isIpAddress(s) || isHostname(s);
}
