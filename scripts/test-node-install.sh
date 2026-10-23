#!/usr/bin/env bash
#
# v1.2.12: checks for the parts of relay-node-install.sh that turn operator
# input into files: argument validation and start.sh generation. Later: the
# download acceleration (source order, fallbacks, limits, version lookup), run
# against a fake curl. The functions
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

# ---- download acceleration: direct, then three GitHub proxies ----
# curl is replaced by a function below, so nothing here touches the network.
# route() decides, per URL, what the fake server does:
#   ok:<body>   200 with that body
#   http:<code> HTTP error (curl -f exits 22)
#   exit:<n>    curl itself fails with that exit status (7 = cannot connect,
#               28 = too slow / timed out)
info() { echo "[INFO] $*"; }
warn() { echo "[WARN] $*"; }
REPO="MoeShinX/relay-panel"
PROXY=""
eval "$(grep -E '^(GH_PREFIXES=|TRUSTED_MIRRORS=|PREFERRED_MIRROR=|MAX_BINARY_BYTES=|MAX_SMALL_BYTES=|GH_LABELS=|EXPECTED_SHA256=)' "$INSTALLER")"

CURL_LOG="$TMP/curl-urls.log"
CURL_ARGS="$TMP/curl-args.log"
curl() {
    local out="" url="" w="" ARGS_LINE="$*"
    while [ $# -gt 0 ]; do
        case "$1" in
            -o) out="$2"; shift 2 ;;
            -w) w="$2"; shift 2 ;;
            --connect-timeout|--max-time|--speed-limit|--speed-time|--retry|--retry-delay|--proxy|-H|--max-filesize) shift 2 ;;
            --retry-connrefused|--progress-bar) shift ;;
            -*) shift ;;
            *) url="$1"; shift ;;
        esac
    done
    echo "$url" >> "$CURL_LOG"
    echo "$ARGS_LINE" >> "$CURL_ARGS"
    local r
    r="$(route "$url")"
    case "$r" in
        ok:*)
            if [ -n "$out" ]; then printf '%s' "${r#ok:}" > "$out"; else printf '%s' "${r#ok:}"; fi
            [ -z "$w" ] || printf '200'
            return 0 ;;
        http:*)
            [ -z "$w" ] || printf '%s' "${r#http:}"
            return 22 ;;
        exit:*)
            [ -z "$w" ] || printf '000'
            return "${r#exit:}" ;;
    esac
}
for fn in mirror_label mirror_order curl_failure_reason valid_elf_file valid_sha256_file \
          file_sha256 valid_program_file fetch_with_mirrors curl_small_text node_version_from_releases \
          node_version_from_cargo resolve_node_version download_binary fetch_checksum; do
    src="$(extract "$fn")"
    [ -n "$src" ] || { echo "[FAIL] $fn not found in the installer" >&2; exit 1; }
    eval "$src"
done

eq() {
    if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: expected [$2], got [$3]"; fi
}
reset_curl() { : > "$CURL_LOG"; : > "$CURL_ARGS"; PREFERRED_MIRROR=-1; }
# Run fetch_with_mirrors in THIS shell (PREFERRED_MIRROR must survive), keeping
# what it printed in $msgs.
run_fetch() {
    local rc=0
    fetch_with_mirrors "$@" > "$TMP/msgs.txt" 2>&1 || rc=$?
    msgs="$(cat "$TMP/msgs.txt")"
    return "$rc"
}
urls() { tr '\n' ' ' < "$CURL_LOG" | sed 's/ $//'; }

ELF=$'\x7fELF fake program bytes'
SHA="$(printf 'ab%.0s' $(seq 1 32))"
API="https://api.github.com/repos/MoeShinX/relay-panel/releases?per_page=30"
RAW="https://raw.githubusercontent.com/MoeShinX/relay-panel/main/crates/node/Cargo.toml"
BIN="https://github.com/MoeShinX/relay-panel/releases/download/node-v1.2.9/relay-node-linux-amd64"
D="https://down.relaypanel.dev/"; G="https://gh-proxy.com/"; F="https://ghfast.top/"

# the list itself
eq "source list is direct, mirror, gh-proxy.com, ghfast.top" \
    "|$D|$G|$F" "$(IFS='|'; echo "${GH_PREFIXES[*]}")"
eq "trusted sources: the first two (direct, mirror)" "2" "$TRUSTED_MIRRORS"
eq "label: direct" "direct" "$(mirror_label 0)"
eq "label: the relay is just 'mirror', never its host" "mirror" "$(mirror_label 1)"
eq "label: public accelerators keep their host name" "gh-proxy.com ghfast.top" "$(mirror_label 2) $(mirror_label 3)"
eq "order: none worked yet" "0 1 2 3" "$(mirror_order -1 4 | tr '\n' ' ' | sed 's/ $//')"
eq "order: the source that worked goes first" "2 0 1 3" "$(mirror_order 2 4 | tr '\n' ' ' | sed 's/ $//')"
eq "order: out of range ignored" "0 1 2 3" "$(mirror_order 9 4 | tr '\n' ' ' | sed 's/ $//')"

# version parsing
RELEASES='[{"tag_name": "v1.2.15"},{"tag_name": "node-v1.2.9"},{"tag_name":"node-v1.2.10"},{"tag_name": "node-v1.1.1"}]'
eq "version from releases JSON: highest node-v*, panel tags ignored" "1.2.10" \
    "$(printf '%s' "$RELEASES" | node_version_from_releases)"
eq "version from releases JSON: none" "" "$(printf '[{"tag_name": "v1.2.15"}]' | node_version_from_releases)"
CARGO='[package]
name = "relay-node"
# comment "9.9.9"
version = "1.2.11"
edition = "2021"

[dependencies]
tokio = { version = "1", features = ["full"] }
'
eq "version from Cargo.toml: [package] only" "1.2.11" "$(printf '%s' "$CARGO" | node_version_from_cargo)"
eq "version from Cargo.toml: workspace-inherited is not guessed" "" \
    "$(printf '[package]\nname = "x"\nversion.workspace = true\n[dependencies]\nfoo = { version = "1.0.0" }\n' | node_version_from_cargo)"
eq "version from Cargo.toml: HTML page is not a version" "" "$(printf '<html>version = "1.2.3"</html>' | node_version_from_cargo)"

# latest-version lookup: TRUSTED sources only.
# API direct -> API via mirror -> Cargo.toml direct -> via mirror
reset_curl
route() { case "$1" in "$API") echo "ok:$RELEASES" ;; *) echo "exit:7" ;; esac; }
resolve_node_version >/dev/null
eq "lookup: GitHub API direct wins" "1.2.10" "$LATEST_NODE"
eq "lookup: ... and nothing else was asked" "$API" "$(urls)"

reset_curl
route() { case "$1" in "$D$API") echo "ok:$RELEASES" ;; "$API") echo "http:403" ;; *) echo "exit:7" ;; esac; }
resolve_node_version >/dev/null
eq "lookup: API refused direct -> via mirror" "1.2.10" "$LATEST_NODE"
eq "lookup: ... in that order" "$API $D$API" "$(urls)"

reset_curl
route() { case "$1" in "$D$RAW") echo "ok:$CARGO" ;; *) echo "http:403" ;; esac; }
resolve_node_version >/dev/null
eq "lookup: APIs refused -> Cargo.toml, direct then mirror" "1.2.11" "$LATEST_NODE"
eq "lookup: ... request order" "$API $D$API $RAW $D$RAW" "$(urls)"

# A public accelerator is never asked, even if it is the only one that would answer
# (it could name any version).
reset_curl
route() { case "$1" in "$G$RAW"|"$F$RAW"|"$G$API"|"$F$API") echo "ok:$CARGO" ;; *) echo "exit:7" ;; esac; }
resolve_node_version >/dev/null
eq "lookup: only public sources would answer -> empty, they were not asked" "" "$LATEST_NODE"
eq "lookup: ... only the two trusted routes were tried" "$API $D$API $RAW $D$RAW" "$(urls)"

reset_curl
route() { echo "exit:7"; }
resolve_node_version >/dev/null
eq "lookup: nothing reachable -> empty, no abort" "" "$LATEST_NODE"
case "$(cat "$CURL_ARGS")" in
    *"--max-filesize 1048576"*) ok "lookup: size cap 1 MB on every request" ;;
    *) bad "lookup: no --max-filesize on the version requests" ;;
esac

# The sha256 (small): trusted sources only, list order, never a public one.
OUT="$TMP/dl.bin"
ELF_SHA="$(printf '%s' "$ELF" | sha256sum | awk '{ print $1 }')"
EVIL=$'\x7fELF tampered program bytes'
EVIL_SHA="$(printf '%s' "$EVIL" | sha256sum | awk '{ print $1 }')"

reset_curl
route() { case "$1" in "$D$BIN.sha256") echo "ok:$SHA  relay-node-linux-amd64" ;; *) echo "exit:7" ;; esac; }
run_fetch "sha256" "$BIN.sha256" "$OUT.sha256" small valid_sha256_file || bad "sha256: expected success via mirror"
eq "sha256: direct then mirror" "$BIN.sha256 $D$BIN.sha256" "$(urls)"
eq "sha256: content" "$SHA  relay-node-linux-amd64" "$(cat "$OUT.sha256")"
eq "sha256: the program's source is not remembered for it" "-1" "$PREFERRED_MIRROR"
if grep -F "$BIN.sha256" "$CURL_ARGS" | tail -n1 | grep -qF -- "--connect-timeout 5 --max-time 10 --max-filesize 1048576"; then
    ok "sha256: 5 s connect, 10 s in all, 1 MB cap"
else
    bad "sha256: wrong limits: $(tail -n1 "$CURL_ARGS")"
fi

# remembered program source must not steer the sha256
reset_curl
PREFERRED_MIRROR=2
route() { case "$1" in "$G$BIN.sha256") echo "ok:$SHA  evil" ;; "$BIN.sha256") echo "ok:$SHA  good" ;; *) echo "exit:7" ;; esac; }
run_fetch "sha256" "$BIN.sha256" "$OUT.sha256" small valid_sha256_file || bad "sha256: expected success on direct"
eq "sha256: a remembered public source is still not asked" "$BIN.sha256" "$(urls)"
eq "sha256: ... and the answer is the trusted one" "$SHA  good" "$(cat "$OUT.sha256")"

# (a)-style: only public sources answer -> failure, public never asked
reset_curl
route() { case "$1" in "$G$BIN.sha256"|"$F$BIN.sha256") echo "ok:$SHA  evil" ;; *) echo "exit:7" ;; esac; }
if run_fetch "sha256" "$BIN.sha256" "$OUT.sha256" small valid_sha256_file; then bad "sha256: must fail when only public sources have it"; else ok "sha256: only public sources answer -> failure"; fi
eq "sha256: ... only direct and mirror were tried" "$BIN.sha256 $D$BIN.sha256" "$(urls)"
[ ! -e "$OUT.sha256" ] && ok "sha256: no leftover file" || bad "sha256: leftover file"

reset_curl
route() { case "$1" in "$D$BIN.sha256") echo "ok:<html>blocked</html>" ;; "$G$BIN.sha256") echo "ok:$SHA  x" ;; *) echo "exit:7" ;; esac; }
if run_fetch "sha256" "$BIN.sha256" "$OUT.sha256" small valid_sha256_file; then bad "sha256: a non-hash must not fall through to a public source"; else ok "sha256: a trusted source's non-hash page does not fall through to a public source"; fi
eq "sha256: ... public not asked" "$BIN.sha256 $D$BIN.sha256" "$(urls)"
case "$msgs" in *"unexpected content"*) ok "sha256: says why" ;; *) bad "sha256: reason missing: $msgs" ;; esac

reset_curl
route() { case "$1" in "$BIN.sha256") echo "ok:<html>blocked</html>" ;; "$D$BIN.sha256") echo "ok:$SHA  x" ;; *) echo "exit:7" ;; esac; }
run_fetch "sha256" "$BIN.sha256" "$OUT.sha256" small valid_sha256_file || bad "sha256: expected the second trusted source to work"
eq "sha256: a non-hash from direct falls through to mirror" "$SHA  x" "$(cat "$OUT.sha256")"

reset_curl
route() { echo "exit:63"; }
run_fetch "sha256" "$BIN.sha256" "$OUT.sha256" small valid_sha256_file || true
case "$msgs" in *"too large"*) ok "sha256: curl exit 63 is reported as too large" ;; *) bad "sha256: reason: $msgs" ;; esac

# The program (large): any source when the sha256 is known, verified against it.
EXPECTED_SHA256="$ELF_SHA"
reset_curl
route() { case "$1" in "$D$BIN") echo "ok:$ELF" ;; *) echo "exit:7" ;; esac; }
if run_fetch "relay-node" "$BIN" "$OUT" large valid_program_file; then ok "binary: direct cannot connect, mirror works"; else bad "binary: expected success via mirror"; fi
eq "binary: content" "$ELF" "$(cat "$OUT")"
eq "binary: remembered the working source" "1" "$PREFERRED_MIRROR"
case "$msgs" in *"direct failed (cannot connect)"*"downloaded via mirror"*) ok "binary: says why it moved on and where it landed" ;; *) bad "binary: messages: $msgs" ;; esac
case "$msgs" in *https://*) bad "binary: a full URL was printed: $msgs" ;; *) ok "binary: no full URL in the output" ;; esac
first="$(head -n1 "$CURL_ARGS")"
case "$first" in
    *"--connect-timeout 5"*"--speed-limit 51200 --speed-time 15 --max-filesize 209715200"*)
        case "$first" in *--max-time*) bad "binary: must have no total time limit: $first" ;; *) ok "binary: 5 s connect, drop below 50 KB/s for 15 s, 200 MB cap, no total limit" ;; esac ;;
    *) bad "binary: wrong limits: $first" ;;
esac

reset_curl
route() { case "$1" in "$F$BIN") echo "ok:$ELF" ;; "$D$BIN") echo "http:404" ;; "$G$BIN") echo "exit:28" ;; *) echo "exit:6" ;; esac; }
run_fetch "relay-node" "$BIN" "$OUT" large valid_program_file || bad "binary: expected success via the last source"
eq "binary: falls through the list in order" "$BIN $D$BIN $G$BIN $F$BIN" "$(urls)"
case "$msgs" in *"HTTP 404"*"too slow or timed out"*) ok "binary: each reason is reported" ;; *) bad "binary: reasons: $msgs" ;; esac

reset_curl
route() { case "$1" in "$D$BIN") echo "ok:<html>blocked</html>" ;; "$G$BIN") echo "ok:" ;; "$F$BIN") echo "ok:$ELF" ;; *) echo "exit:7" ;; esac; }
run_fetch "relay-node" "$BIN" "$OUT" large valid_program_file || bad "binary: expected success after skipping bad content"
case "$msgs" in *"unexpected content"*"empty response"*) ok "binary: an error page or empty body counts as a failed source" ;; *) bad "binary: messages: $msgs" ;; esac
eq "binary: ... and the next source is used" "3" "$PREFERRED_MIRROR"

# (c) a tampered program (valid ELF, wrong hash) from a public source: rejected, next source tried
reset_curl
route() { case "$1" in "$G$BIN") echo "ok:$EVIL" ;; "$F$BIN") echo "ok:$ELF" ;; *) echo "exit:7" ;; esac; }
run_fetch "relay-node" "$BIN" "$OUT" large valid_program_file || bad "binary: expected success from the next source"
case "$msgs" in *"gh-proxy.com failed (sha256 mismatch)"*) ok "binary: tampered copy from a public source is rejected as a sha256 mismatch" ;; *) bad "binary: messages: $msgs" ;; esac
eq "binary: ... the genuine copy from the next source is what is kept" "$ELF" "$(cat "$OUT")"
eq "binary: ... and the sha256 was never requested while downloading" "0" "$(grep -c '\.sha256$' "$CURL_LOG" || true)"

reset_curl
route() { case "$1" in "$G$BIN"|"$F$BIN") echo "ok:$EVIL" ;; *) echo "exit:7" ;; esac; }
if run_fetch "relay-node" "$BIN" "$OUT" large valid_program_file; then bad "binary: only tampered copies -> must fail"; else ok "binary: only tampered copies available -> failure"; fi
[ ! -e "$OUT" ] && ok "binary: no leftover file" || bad "binary: leftover file after failure"

reset_curl
PREFERRED_MIRROR=2
route() { case "$1" in "$BIN") echo "ok:$ELF" ;; *) echo "http:404" ;; esac; }
run_fetch "relay-node" "$BIN" "$OUT" large valid_program_file || bad "binary: expected success on direct"
eq "binary: a remembered source that fails is followed by the rest in list order" "$G$BIN $BIN" "$(urls)"
eq "binary: and the new working source is remembered" "0" "$PREFERRED_MIRROR"

reset_curl
route() { echo "http:404"; }
if run_fetch "relay-node" "$BIN" "$OUT" large valid_program_file; then bad "binary: all 404 should fail"; else ok "binary: every source 404 -> failure"; fi
eq "binary: every source was tried once" "$BIN $D$BIN $G$BIN $F$BIN" "$(urls)"

# without a sha256 to check against (SKIP_CHECKSUM), public sources are not used at all
EXPECTED_SHA256=""
reset_curl
route() { case "$1" in "$G$BIN"|"$F$BIN") echo "ok:$EVIL" ;; *) echo "exit:7" ;; esac; }
if run_fetch "relay-node" "$BIN" "$OUT" large valid_program_file; then bad "binary: unverified copy from a public source accepted"; else ok "binary: no sha256 -> a public source is not used"; fi
eq "binary: ... only direct and mirror were asked" "$BIN $D$BIN" "$(urls)"
EXPECTED_SHA256="$ELF_SHA"

# a custom mirror (RELAY_NODE_BASE_URL) is used alone
CURL_OPTS=(-fL --progress-bar --connect-timeout 10 --max-time 120)
MIRROR_BIN="https://mirror.example.com/relay-node-linux-amd64"
reset_curl
route() { echo "exit:7"; }
if download_binary "$BIN" "$MIRROR_BIN" "$OUT" >/dev/null 2>&1; then bad "custom mirror: should fail when the mirror is down"; else ok "custom mirror: its failure is a failure (no silent fallback)"; fi
eq "custom mirror: only the mirror was contacted" "$MIRROR_BIN" "$(urls)"
reset_curl
route() { echo "exit:7"; }
fetch_checksum "$BIN.sha256" "$MIRROR_BIN.sha256" "$OUT.sha256" >/dev/null 2>&1 || true
eq "custom mirror: its sha256 comes from the mirror only" "$MIRROR_BIN.sha256" "$(urls)"

# ---- the installer script itself, end to end, with fake curl/uname/id/systemctl ----
# A copy of the installer with /opt and /etc/systemd/system moved into $TMP, run as
# a fake root on a fake x86-64 Linux. Nothing real is touched.
FAKEBIN="$TMP/fakebin"; mkdir -p "$FAKEBIN"
FAKE_LOG="$TMP/fake-curl.log"; FAKE_ROUTES="$TMP/fake-routes.tsv"
cat > "$FAKEBIN/curl" <<'FAKECURL'
#!/usr/bin/env bash
out=""; url=""; w=""
while [ $# -gt 0 ]; do
    case "$1" in
        -o) out="$2"; shift 2 ;;
        -w) w="$2"; shift 2 ;;
        --connect-timeout|--max-time|--speed-limit|--speed-time|--retry|--retry-delay|--proxy|-H|--max-filesize) shift 2 ;;
        -*) shift ;;
        *) url="$1"; shift ;;
    esac
done
echo "$url" >> "$FAKE_LOG"
action="$(awk -F'\t' -v u="$url" '$1 == u { print $2; exit }' "$FAKE_ROUTES")"
case "${action:-exit:7}" in
    body:*) cp "${action#body:}" "$out"; [ -z "$w" ] || printf '200'; exit 0 ;;
    http:*) [ -z "$w" ] || printf '%s' "${action#http:}"; exit 22 ;;
    exit:*) [ -z "$w" ] || printf '000'; exit "${action#exit:}" ;;
esac
FAKECURL
cat > "$FAKEBIN/uname" <<'FAKEUNAME'
#!/usr/bin/env bash
case "${1:-}" in -s) echo Linux ;; -m) echo x86_64 ;; *) echo Linux ;; esac
FAKEUNAME
printf '#!/usr/bin/env bash\necho 0\n' > "$FAKEBIN/id"
printf '#!/usr/bin/env bash\nexit 0\n' > "$FAKEBIN/systemctl"
printf '#!/usr/bin/env bash\nexit 0\n' > "$FAKEBIN/sleep"
chmod +x "$FAKEBIN"/*

INSTALLER_COPY="$TMP/installer-copy.sh"
sed -e "s#INSTALL_DIR=\"/opt/#INSTALL_DIR=\"$TMP/opt/#" \
    -e "s#SERVICE_FILE=\"/etc/systemd/system/#SERVICE_FILE=\"$TMP/etc-systemd-#" \
    "$INSTALLER" > "$INSTALLER_COPY"
grep -q "$TMP/opt/" "$INSTALLER_COPY" && grep -q "$TMP/etc-systemd-" "$INSTALLER_COPY" \
    || { echo "[FAIL] could not redirect the installer's paths" >&2; exit 1; }

# fake releases: a genuine program (>100 kB, ELF), a tampered one, their hashes
GENUINE="$TMP/genuine.bin"; TAMPERED="$TMP/tampered.bin"
{ printf '\x7fELF'; head -c 200000 /dev/zero | tr '\0' 'g'; } > "$GENUINE"
{ printf '\x7fELF'; head -c 200000 /dev/zero | tr '\0' 'x'; } > "$TAMPERED"
GENUINE_SHA="$(sha256sum "$GENUINE" | awk '{ print $1 }')"
TAMPERED_SHA="$(sha256sum "$TAMPERED" | awk '{ print $1 }')"
printf '%s  relay-node-linux-amd64\n' "$GENUINE_SHA" > "$TMP/genuine.sha256"
printf '%s  relay-node-linux-amd64\n' "$TAMPERED_SHA" > "$TMP/tampered.sha256"
BIN="https://github.com/MoeShinX/relay-panel/releases/download/node-v1.2.9/relay-node-linux-amd64"

# routes: lines of "<url>TAB<action>"; anything not listed -> curl exit 7
set_routes() { : > "$FAKE_ROUTES"; local u a; while [ $# -ge 2 ]; do printf '%s\t%s\n' "$1" "$2" >> "$FAKE_ROUTES"; shift 2; done; }
run_installer() {
    # args: extra env assignments are passed by the caller via `env`
    rm -rf "$TMP/opt"; : > "$FAKE_LOG"
    PATH="$FAKEBIN:$PATH" FAKE_LOG="$FAKE_LOG" FAKE_ROUTES="$FAKE_ROUTES" \
        bash "$INSTALLER_COPY" -t tok-123 -u http://203.0.113.10:18888 -s inst --version 1.2.9 \
        > "$TMP/run.out" 2>&1
}
asked() { cat "$FAKE_LOG" | tr '\n' ' ' | sed 's/ $//'; }

# (b) public source carries the program, a trusted one gives the hash -> installed
set_routes "$D$BIN.sha256" "body:$TMP/genuine.sha256" "$D$BIN" "http:404" "$G$BIN" "body:$GENUINE"
if run_installer; then ok "installer: program from a public source + hash from mirror -> installed"; else bad "installer: expected success: $(tail -n5 "$TMP/run.out")"; fi
if cmp -s "$TMP/opt/inst/relay-node" "$GENUINE"; then ok "installer: the installed program is the verified one"; else bad "installer: wrong program installed"; fi
eq "installer: hash asked of direct then mirror only, and before any program" \
    "$BIN.sha256 $D$BIN.sha256 $BIN $D$BIN $G$BIN" "$(asked)"

# (a) the hash is available from NO trusted source, a public source offers program + matching hash
set_routes "$G$BIN.sha256" "body:$TMP/tampered.sha256" "$G$BIN" "body:$TAMPERED" "$F$BIN.sha256" "body:$TMP/tampered.sha256" "$F$BIN" "body:$TAMPERED"
if run_installer; then bad "installer: installed although no trusted source gave the sha256"; else ok "installer: no trusted sha256 -> refuses to install (fails closed)"; fi
eq "installer: ... nothing but the two trusted hash URLs was requested" "$BIN.sha256 $D$BIN.sha256" "$(asked)"
if [ ! -e "$TMP/opt/inst/relay-node" ] && [ ! -e "$TMP/opt/inst/relay-node.tmp" ]; then ok "installer: ... no program file left behind"; else bad "installer: a program file exists"; fi
if grep -q "Could not get the sha256" "$TMP/run.out" && grep -q "direct, mirror" "$TMP/run.out" \
    && grep -q "RELAY_NODE_BASE_URL" "$TMP/run.out" && grep -q "SKIP_CHECKSUM" "$TMP/run.out"; then
    ok "installer: ... the message says what was tried and what to do"
else
    bad "installer: unclear message: $(tail -n12 "$TMP/run.out")"
fi
if grep -Eq 'https://(down\.relaypanel\.dev|gh-proxy\.com|ghfast\.top)/' "$TMP/run.out"; then bad "installer: a proxied download link was printed"; else ok "installer: ... no proxied download link in the output"; fi

# (c) tampered program from the first public source, genuine from the next
set_routes "$BIN.sha256" "body:$TMP/genuine.sha256" "$D$BIN" "http:404" "$G$BIN" "body:$TAMPERED" "$F$BIN" "body:$GENUINE"
if run_installer; then ok "installer: tampered copy from one public source, genuine from the next -> installed"; else bad "installer: expected success: $(tail -n5 "$TMP/run.out")"; fi
if cmp -s "$TMP/opt/inst/relay-node" "$GENUINE"; then ok "installer: ... and the genuine one is installed"; else bad "installer: wrong program installed"; fi
grep -q "gh-proxy.com failed (sha256 mismatch)" "$TMP/run.out" && ok "installer: ... the mismatch is reported" || bad "installer: mismatch not reported"

set_routes "$BIN.sha256" "body:$TMP/genuine.sha256" "$D$BIN" "http:404" "$G$BIN" "body:$TAMPERED" "$F$BIN" "body:$TAMPERED"
if run_installer; then bad "installer: installed a tampered program"; else ok "installer: only tampered copies -> refuses to install"; fi
[ ! -e "$TMP/opt/inst/relay-node" ] && ok "installer: ... nothing installed" || bad "installer: a program was installed"

# SKIP_CHECKSUM=1 is the explicit opt-out: no hash request, and no third-party accelerator
set_routes "$G$BIN" "body:$TAMPERED" "$F$BIN" "body:$TAMPERED" "$D$BIN" "body:$GENUINE"
if (export SKIP_CHECKSUM=1; run_installer); then ok "installer: SKIP_CHECKSUM=1 installs from GitHub itself"; else bad "installer: SKIP_CHECKSUM run failed: $(tail -n5 "$TMP/run.out")"; fi
eq "installer: ... only direct and mirror were used, no hash asked" "$BIN $D$BIN" "$(asked)"

# RELAY_NODE_BASE_URL: used alone, as before (its own hash, nothing from GitHub)
set_routes "https://mirror.example.com/relay-node-linux-amd64" "body:$GENUINE" \
           "https://mirror.example.com/relay-node-linux-amd64.sha256" "body:$TMP/genuine.sha256"
if (export RELAY_NODE_BASE_URL=https://mirror.example.com; run_installer); then ok "installer: custom mirror works as before"; else bad "installer: custom mirror run failed: $(tail -n5 "$TMP/run.out")"; fi
eq "installer: ... only the mirror was contacted" "https://mirror.example.com/relay-node-linux-amd64 https://mirror.example.com/relay-node-linux-amd64.sha256" "$(asked)"
set_routes
if (export RELAY_NODE_BASE_URL=https://mirror.example.com; run_installer); then bad "installer: custom mirror down should fail"; else ok "installer: custom mirror down -> failure, no fallback to GitHub"; fi
eq "installer: ... still only the mirror was contacted" "https://mirror.example.com/relay-node-linux-amd64" "$(asked)"

[ "$FAILED" -eq 0 ] || { echo "node installer checks FAILED" >&2; exit 1; }
echo "all node installer checks passed"
