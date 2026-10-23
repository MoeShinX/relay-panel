/** The node installer, straight from the repository. */
export const INSTALL_SCRIPT_URL =
  'https://raw.githubusercontent.com/MoeShinX/relay-panel/main/scripts/relay-node-install.sh';

/**
 * Where the installer script may be fetched from, tried in this order (5 s to
 * connect, 20 s in all, 2 MB at most each): GitHub directly, then
 * the trusted relay. Deliberately NOT the public third-party accelerators the
 * installer itself may use for the program file: this is a SCRIPT that is run
 * as root and cannot be checked against a hash, so only these two are trusted.
 */
export const INSTALL_TRUSTED_PREFIXES = ['https://down.relaypanel.dev/'] as const;

/**
 * One pasteable bash line: fetch the installer directly, else through
 * the trusted relay, then run it with the token and panel address.
 *
 * It goes through a temp file, not `bash <(curl ...)`, so a download that dies
 * half way (curl exits non-zero) is thrown away and the next source tried,
 * instead of bash running a truncated script or two scripts glued together.
 */
export function buildInstallCommand(token: string, panelUrl: string): string {
  const prefixes = ["''", ...INSTALL_TRUSTED_PREFIXES].join(' ');
  return (
    `f=$(mktemp); for p in ${prefixes}; do ` +
    `curl -fsSL --connect-timeout 5 --max-time 20 --max-filesize 2097152 -o "$f" "\${p}${INSTALL_SCRIPT_URL}" && break; ` +
    `done && bash "$f" -t ${token} -u ${panelUrl}; rm -f "$f"`
  );
}
