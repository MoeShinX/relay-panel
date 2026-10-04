#!/usr/bin/env bash
#
# v1.2.12: checks for the parts of relay-node-install.sh that turn operator
# input into files: argument validation and start.sh generation. The functions
# are loaded straight out of the installer, so this tests the real code without
# running the (root-only, networked) install itself.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
INSTALLER="$ROOT/scripts/relay-node-install.sh"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

FAILED=0
ok()  { echo "[OK]   $*"; }
bad() { echo "[FAIL] $*" >&2; FAILED=1; }

# The installer's own fail() exits; keep that so a rejected value ends the
# subshell it is checked in.
fail() { echo "$*" >&2; exit 1; }

extract() {
    awk -v name="$1" '$0 ~ "^" name "\\(\\) \\{" {on=1} on {print} on && /^}/ {exit}' "$INSTALLER"
}
for fn in validate_install_args write_start_sh; do
    src="$(extract "$fn")"
    [ -n "$src" ] || { echo "[FAIL] $fn not found in the installer" >&2; exit 1; }
    eval "$src"
done

GOOD_TOKEN="3f2b8c1e-5a4d-4e6f-9b7a-1c2d3e4f5a6b"
GOOD_URL="http://203.0.113.10:18888"

accepts() {
    if (validate_install_args "$1" "$2" "$3") >/dev/null 2>&1; then ok "accepts: $4"; else bad "rejected: $4"; fi
}
rejects() {
    if (validate_install_args "$1" "$2" "$3") >/dev/null 2>&1; then bad "accepted: $4"; else ok "rejects: $4"; fi
}

accepts "$GOOD_TOKEN" "$GOOD_URL" relay-node "uuid token + http url"
accepts "$GOOD_TOKEN" "https://panel.example.com/" relay-node-hk "https url + custom service name"

# shellcheck disable=SC2016 # the $(...) must stay literal: it is the payload
rejects 'abc$(id)' "$GOOD_URL" relay-node "command substitution in token"
rejects "abc def" "$GOOD_URL" relay-node "space in token"
rejects "abc|def" "$GOOD_URL" relay-node "sed delimiter in token"
rejects "" "$GOOD_URL" relay-node "empty token"
# shellcheck disable=SC2016
rejects "$GOOD_TOKEN" 'http://x/$(touch /tmp/pwned)' relay-node "command substitution in url"
rejects "$GOOD_TOKEN" 'http://x/`id`' relay-node "backticks in url"
rejects "$GOOD_TOKEN" 'http://x/"' relay-node "double quote in url"
rejects "$GOOD_TOKEN" "http://x/'" relay-node "single quote in url"
rejects "$GOOD_TOKEN" 'http://x/\' relay-node "backslash in url"
rejects "$GOOD_TOKEN" "http://x y" relay-node "space in url"
rejects "$GOOD_TOKEN" "ftp://x" relay-node "non-http scheme"
rejects "$GOOD_TOKEN" "http://" relay-node "url without host"
rejects "$GOOD_TOKEN" "$GOOD_URL" "../etc" "path traversal in service name"
rejects "$GOOD_TOKEN" "$GOOD_URL" "a/b" "slash in service name"
rejects "$GOOD_TOKEN" "$GOOD_URL" ".hidden" "leading dot in service name"
rejects "$GOOD_TOKEN" "$GOOD_URL" "" "empty service name"

# ---- start.sh: hostile values stay literal, and it runs from its own dir ----
# Even with validation bypassed, nothing in the values may execute.
INST="$TMP/opt/relay node-hk"   # a space in the path must survive too
mkdir -p "$INST"
cat > "$INST/relay-node" <<'SH'
#!/usr/bin/env bash
printf '%s\n' "$PWD" "$RELAY_NODE_DIR" "$PANEL_URL" "$NODE_TOKEN" "${FROM_ENV_FILE:-unset}"
SH
chmod +x "$INST/relay-node"
echo 'FROM_ENV_FILE=yes' > "$INST/relay-node.env"

HOSTILE_URL="http://p:1/\$(touch $TMP/pwned-url)\`touch $TMP/pwned-tick\`\"'|&"
HOSTILE_TOKEN="t\";touch $TMP/pwned-token;\"|&\\"
write_start_sh "$INST/start.sh" "$INST" "$HOSTILE_URL" "$HOSTILE_TOKEN"

if bash -n "$INST/start.sh"; then ok "start.sh is valid bash"; else bad "start.sh has a syntax error"; fi

out="$(cd / && bash "$INST/start.sh" 2>&1)" || bad "start.sh failed to run: $out"
expected="$(printf '%s\n' "$INST" "$INST" "$HOSTILE_URL" "$HOSTILE_TOKEN" yes)"
if [ "$out" = "$expected" ]; then
    ok "start.sh passes the values through literally, from its own dir, with its own env file"
else
    bad "start.sh output mismatch"
    diff <(echo "$expected") <(echo "$out") >&2 || true
fi

if ls "$TMP"/pwned-* >/dev/null 2>&1; then
    bad "a payload in the URL/token executed"
else
    ok "no payload executed"
fi

if grep -q '/opt/relay-node' "$INST/start.sh"; then
    bad "start.sh still hardcodes /opt/relay-node"
else
    ok "start.sh has no hardcoded /opt/relay-node"
fi

[ "$FAILED" -eq 0 ] || { echo "node installer checks FAILED" >&2; exit 1; }
echo "all node installer checks passed"
