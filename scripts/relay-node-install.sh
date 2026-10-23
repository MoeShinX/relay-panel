#!/usr/bin/env bash
#
# RelayPanel node installer - downloads and runs relay-node as a systemd service.
#
# Usage:
#   bash <(curl -fsSL https://raw.githubusercontent.com/MoeShinX/relay-panel/main/scripts/relay-node-install.sh) \
#     -t <NODE_TOKEN> -u <PANEL_URL>
#
# Options:
#   -t, --token         Node token (required, from the panel UI)
#   -u, --url           Panel URL, e.g. http://panel-ip:18888 (required)
#   -s, --service-name  systemd service name (default: relay-node)
#   -p, --proxy         Proxy for downloads, e.g. socks5://127.0.0.1:10808
#                       (or set RELAY_PROXY env var)
#
# Environment:
#   RELAY_PROXY           Same as -p (e.g. socks5://127.0.0.1:10808)
#   RELAY_NODE_BASE_URL   Custom download mirror base, e.g. https://download.example.com/relay-panel
#                         The script will fetch {BASE_URL}/relay-node-linux-{arch}
#                         When set it is the ONLY source (no GitHub, no accelerators).
#
# Download acceleration (default, when RELAY_NODE_BASE_URL is not set):
#   Everything from GitHub is fetched directly first; if that cannot connect (5 s)
#   or is too slow, the script moves on, in this order, to a trusted relay,
#   then https://gh-proxy.com/ and https://ghfast.top/.
#   Only direct GitHub and the trusted relay are trusted for the latest-version
#   lookup and for the program's sha256 (GitHub's own .sha256 file, fetched
#   BEFORE the program). gh-proxy.com and ghfast.top may only carry the program
#   file, which must then match that sha256 exactly. If the sha256 cannot be
#   obtained from a trusted source the install stops. Each switch is printed.
#
# Idempotent re-runs: downloading to a temp file and swapping atomically means
# this script can be re-run to upgrade an already-running node. The running
# binary is only replaced AFTER the new one is fully downloaded and validated,
# so a failed download never breaks an existing install.
#
set -euo pipefail

# v1.0.9: move off the invocation directory immediately. If the script is run
# from a directory that has since been deleted (common when piping via
# `bash <(curl ...)` from a scratch dir), child processes inherit a dead cwd and
# fail with "shell-init: error retrieving current directory: getcwd". Anchoring
# to / (all work uses absolute paths anyway) avoids that class of error.
cd / 2>/dev/null || true

# Bump this when releasing a new version. The binary is downloaded from
# GitHub Releases assets.
SCRIPT_VERSION="1.2.10"
REPO="MoeShinX/relay-panel"

GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m'

info()  { echo -e "${GREEN}[INFO]${NC}  $*"; }
warn()  { echo -e "${YELLOW}[WARN]${NC}  $*"; }
fail()  { echo -e "${RED}[FAIL]${NC}  $*"; exit 1; }

# v1.2.12: validate the values that end up in start.sh, the systemd unit and
# /opt/<name>. Before, -u/-t were spliced into start.sh with sed, so a value
# containing $(...) ran on every node start and a "|" or "&" broke the file.
validate_install_args() {
    # $1 = token, $2 = panel url, $3 = service name
    case "$1" in
        ''|*[!A-Za-z0-9._-]*) fail "Invalid token: only letters, digits, '.', '_' and '-' are allowed. Copy it again from the panel." ;;
    esac
    case "$2" in
        http://?*|https://?*) ;;
        *) fail "Invalid panel URL: $2 (expected http://host:port or https://host)" ;;
    esac
    case "$2" in
        *[[:space:]\"\'\`\$\\]*) fail "Invalid panel URL: it must not contain spaces, quotes, '\$', '\`' or '\\'." ;;
    esac
    case "$3" in
        ''|.*|*[!A-Za-z0-9._-]*) fail "Invalid service name: $3 (letters, digits, '.', '_' and '-'; must not start with '.')" ;;
    esac
}

# Write the node's start script. Values are shell-quoted with printf %q (never
# substituted into the text), so whatever they contain stays a literal string.
write_start_sh() {
    # $1 = start.sh path, $2 = install dir, $3 = panel url, $4 = token
    {
        printf '#!/usr/bin/env bash\nset -euo pipefail\n'
        printf 'cd %q\n' "$2"
        printf 'export RELAY_NODE_DIR=%q\n' "$2"
        printf 'export PANEL_URL=%q\n' "$3"
        printf 'export NODE_TOKEN=%q\n' "$4"
        cat <<'STARTEOF'
export POLL_INTERVAL="${POLL_INTERVAL:-10}"
export RUST_LOG="${RUST_LOG:-info}"
# Optional config sourced from relay-node.env (next to this script) if present
# (written by the installer with commented examples; edit it to set
# LISTEN_IPV4/LISTEN_IPV6, OUTBOUND_INTERFACE/OUTBOUND_BIND_IPV4 or
# SHUTDOWN_DRAIN_SECS). If the file doesn't exist, all of these stay unset and
# the node uses its defaults (dual-stack listen, system-routed egress).
if [ -f ./relay-node.env ]; then
    set -a
    . ./relay-node.env
    set +a
fi
exec ./relay-node
STARTEOF
    } > "$1"
}

# ---------- Defaults ----------
NODE_TOKEN=""
PANEL_URL=""
SERVICE_NAME="relay-node"
PROXY="${RELAY_PROXY:-}"
# v1.2: explicit node version override (--version X.Y.Z). When empty, the
# script queries GitHub for the latest node-v* tag and falls back to
# SCRIPT_VERSION if the query fails (it NEVER guesses the panel version).
TARGET_VERSION=""

# ---------- Parse args ----------
while [[ $# -gt 0 ]]; do
    case "$1" in
        -t|--token)         NODE_TOKEN="$2"; shift 2 ;;
        -u|--url)           PANEL_URL="$2"; shift 2 ;;
        -s|--service-name)  SERVICE_NAME="$2"; shift 2 ;;
        -p|--proxy)         PROXY="$2"; shift 2 ;;
        --version)          TARGET_VERSION="$2"; shift 2 ;;
        -h|--help)
            echo "Usage: $0 -t <token> -u <panel-url> [-s <service-name>] [-p <proxy>] [--version X.Y.Z]"
            echo ""
            echo "Options:"
            echo "  -t, --token         Node token from the panel UI (required)"
            echo "  -u, --url           Panel URL, e.g. http://panel-ip:18888 (required)"
            echo "  -s, --service-name  systemd service name (default: relay-node)"
            echo "  -p, --proxy         Download proxy, e.g. socks5://127.0.0.1:10808"
            echo "  --version X.Y.Z     Install a specific node version (default: latest node-v* from GitHub)"
            echo ""
            echo "Environment:"
            echo "  RELAY_PROXY           Same as -p"
            echo "  RELAY_NODE_BASE_URL   Custom mirror for binary downloads (used alone: no GitHub, no accelerators)"
            echo ""
            echo "Downloads from GitHub try direct first, then a trusted mirror, gh-proxy.com and"
            echo "ghfast.top in turn (5 s to connect; the program is dropped if under 50 KB/s for 15 s)."
            echo "The sha256 and the latest version are only taken from GitHub itself (direct or"
            echo "through the trusted mirror); gh-proxy.com and ghfast.top only carry the program file."
            exit 0
            ;;
        *)
            fail "Unknown option: $1. Use -h for help."
            ;;
    esac
done

# ---------- Validate ----------
if [ -z "$NODE_TOKEN" ]; then
    fail "Missing required option: -t/--token. Get it from the panel's Device Groups page."
fi
if [ -z "$PANEL_URL" ]; then
    fail "Missing required option: -u/--url. Example: http://203.0.113.10:18888"
fi
validate_install_args "$NODE_TOKEN" "$PANEL_URL" "$SERVICE_NAME"

# ---------- Platform check ----------
if [ "$(uname -s)" != "Linux" ]; then
    fail "This installer only runs on Linux. Current OS: $(uname -s)"
fi
if [ "$(id -u)" -ne 0 ]; then
    fail "Please run as root (use sudo)."
fi

# ---------- Architecture detection ----------
ARCH_RAW="$(uname -m)"
case "$ARCH_RAW" in
    x86_64|amd64) ARCH="amd64" ;;
    aarch64|arm64) ARCH="arm64" ;;
    *)
        fail "Unsupported architecture: $ARCH_RAW. Only amd64 and arm64 are supported."
        ;;
esac
info "Detected architecture: $ARCH ($ARCH_RAW)"

# ---------- Install dirs ----------
INSTALL_DIR="/opt/${SERVICE_NAME}"
BINARY="${INSTALL_DIR}/relay-node"
# Temp download target. We never curl directly onto $BINARY because if the
# service is running, the kernel refuses writes to the executing file with
# ETXTBSY ("Text file busy"). Downloading to .tmp sidesteps that entirely;
# the atomic mv happens only after the service is stopped.
TMP_BINARY="${BINARY}.tmp"

# ---------- GitHub download acceleration ----------
# GitHub is slow or blocked in some networks, so everything fetched from GitHub
# is tried in this order, and the first source that works is used:
#   1. direct                          (trusted)
#   2. the trusted relay, "mirror"     (trusted)
#   3. https://gh-proxy.com/           (public third party)
#   4. https://ghfast.top/             (public third party)
# A proxy URL is the proxy prefix followed by the full GitHub URL.
#
# Trust: the sha256 file and the latest-version lookup are taken ONLY from the
# trusted sources (the first TRUSTED_MIRRORS entries), never from a public one
# and never because "it served the program": a relay that supplies both the file
# and its hash proves nothing. The sha256 is fetched FIRST; if no trusted source
# gives it the install stops. The program may come from any source and must match
# that sha256 exactly (a mismatch just moves on to the next source).
#
# Limits: 5 s to connect; small files (version, sha256) 10 s in all per source and
# at most 1 MB; the program has no total time limit but is dropped for the next
# source if it stays under 50 KB/s for 15 s, and is capped at 200 MB. Within one
# run, once a source delivered the program it is tried first for the next one.
# RELAY_NODE_BASE_URL (a mirror you run yourself) overrides all of this.
GH_PREFIXES=("" "https://down.relaypanel.dev/" "https://gh-proxy.com/" "https://ghfast.top/")
# What messages call each source (never a URL); same order as GH_PREFIXES.
GH_LABELS=("direct" "mirror" "gh-proxy.com" "ghfast.top")
# GH_PREFIXES[0 .. TRUSTED_MIRRORS-1] are the trusted ones.
TRUSTED_MIRRORS=2
# Index into GH_PREFIXES of the source that last delivered the program (-1 = none yet).
PREFERRED_MIRROR=-1
# Size caps passed to curl --max-filesize.
MAX_BINARY_BYTES=209715200
MAX_SMALL_BYTES=1048576
# sha256 (lowercase hex) the program must match; set once the trusted sha256 is known.
EXPECTED_SHA256=""

# Short name of a source for messages (never a full URL).
mirror_label() {
    # $1 = index into GH_PREFIXES
    echo "${GH_LABELS[$1]}"
}

# Indexes 0..n-1 in the order to try: the preferred one first, then the rest.
mirror_order() {
    # $1 = preferred index (-1 = none), $2 = count
    local p="$1" n="$2" i
    if [ "$p" -ge 0 ] && [ "$p" -lt "$n" ]; then
        echo "$p"
    else
        p=-1
    fi
    for ((i = 0; i < n; i++)); do
        if [ "$i" -ne "$p" ]; then
            echo "$i"
        fi
    done
    return 0
}

# Why a curl run failed, in a few words (no URLs).
curl_failure_reason() {
    # $1 = curl exit status, $2 = HTTP status curl reported
    case "$1" in
        6)  echo "cannot resolve host" ;;
        7)  echo "cannot connect" ;;
        22) echo "HTTP ${2:-error}" ;;
        28) echo "too slow or timed out" ;;
        63) echo "too large" ;;
        35|51|52|53|54|58|59|60|77|83|90|91) echo "TLS/connection error" ;;
        18|56) echo "connection broke while downloading" ;;
        *)  echo "curl error $1" ;;
    esac
}

# Content checks, so a 200 "blocked" page from a proxy counts as that source failing.
valid_elf_file() {
    # $1 = file. ELF magic 7f 45 4c 46.
    local magic
    magic="$(head -c 4 "$1" 2>/dev/null | od -A n -t x1 2>/dev/null | tr -d ' \n')"
    [ "$magic" = "7f454c46" ]
}
valid_sha256_file() {
    # $1 = file. First word must be 64 hex digits.
    local h
    h="$(awk '{ print $1; exit }' "$1" 2>/dev/null | tr -d '[:space:]')"
    [ "${#h}" -eq 64 ] && [ -z "${h//[0-9A-Fa-f]/}" ]
}

# sha256 of a file as lowercase hex ("" when no tool is available).
file_sha256() {
    # $1 = file
    local h=""
    if command -v sha256sum >/dev/null 2>&1; then
        h="$(sha256sum "$1" 2>/dev/null | awk '{ print $1 }')"
    elif command -v shasum >/dev/null 2>&1; then
        h="$(shasum -a 256 "$1" 2>/dev/null | awk '{ print $1 }')"
    elif command -v sha256 >/dev/null 2>&1; then
        h="$(sha256 "$1" 2>/dev/null | awk '{ print $1 }')"
    fi
    printf '%s' "$h" | tr '[:upper:]' '[:lower:]'
}

# The program: an ELF file that matches EXPECTED_SHA256 (when known).
# Returns 1 for "not an ELF", 2 for "sha256 mismatch".
valid_program_file() {
    # $1 = file
    local a
    valid_elf_file "$1" || return 1
    if [ -n "$EXPECTED_SHA256" ]; then
        a="$(file_sha256 "$1")"
        if [ -n "$a" ] && [ "$a" != "$EXPECTED_SHA256" ]; then
            return 2
        fi
    fi
    return 0
}

# Download a GitHub URL to a file through the source list.
fetch_with_mirrors() {
    # $1 = what (for messages), $2 = full GitHub URL, $3 = destination file,
    # $4 = "small" | "large", $5 = validator function name.
    #   small: TRUSTED sources only, list order (sha256, version text).
    #   large: the program; every source (preferred one first) when EXPECTED_SHA256
    #          is known, otherwise (SKIP_CHECKSUM) trusted sources only.
    # Returns 0 on success; 1 when every source failed (dest removed).
    local what="$1" url="$2" dest="$3" mode="$4" validator="$5"
    local i label rc code why vrc order
    local -a opts
    if [ "$mode" = "large" ] && [ -n "$EXPECTED_SHA256" ]; then
        order="$(mirror_order "$PREFERRED_MIRROR" "${#GH_PREFIXES[@]}")"
    elif [ "$mode" = "large" ]; then
        warn "${what}: no sha256 to check against; using GitHub itself only (no third-party accelerators)"
        order="$(mirror_order -1 "$TRUSTED_MIRRORS")"
    else
        order="$(mirror_order -1 "$TRUSTED_MIRRORS")"
    fi
    for i in $order; do
        label="$(mirror_label "$i")"
        if [ "$mode" = "large" ]; then
            opts=(--progress-bar --connect-timeout 5 --speed-limit 51200 --speed-time 15 --max-filesize "$MAX_BINARY_BYTES")
        else
            opts=(-s --connect-timeout 5 --max-time 10 --max-filesize "$MAX_SMALL_BYTES")
        fi
        rm -f "$dest"
        rc=0
        code="$(curl -fL "${opts[@]}" ${PROXY:+--proxy "$PROXY"} \
            -o "$dest" -w '%{http_code}' "${GH_PREFIXES[$i]}${url}")" || rc=$?
        if [ "$rc" -eq 0 ]; then
            if [ ! -s "$dest" ]; then
                why="empty response"
            else
                vrc=0
                "$validator" "$dest" || vrc=$?
                case "$vrc" in
                    0)
                        info "${what}: downloaded via ${label}"
                        if [ "$mode" = "large" ]; then
                            PREFERRED_MIRROR="$i"
                        fi
                        return 0
                        ;;
                    2) why="sha256 mismatch" ;;
                    *) why="unexpected content" ;;
                esac
            fi
        else
            why="$(curl_failure_reason "$rc" "$code")"
        fi
        warn "${what}: ${label} failed (${why}); trying the next source"
    done
    rm -f "$dest"
    return 1
}

# Small text via curl to stdout; empty output on any failure.
curl_small_text() {
    # $1 = URL
    curl -fsSL --connect-timeout 5 --max-time 10 --max-filesize "$MAX_SMALL_BYTES" \
        ${PROXY:+--proxy "$PROXY"} -H 'User-Agent: relay-node-install' "$1" 2>/dev/null || true
}

# Highest node-v* version in a GitHub releases JSON on stdin. jq is not assumed.
node_version_from_releases() {
    { grep -oE '"tag_name": ?"node-v[0-9]+\.[0-9]+\.[0-9]+[^"]*"' \
        | sed -E 's/"tag_name": ?"node-v//; s/"$//' \
        | grep -E '^[0-9]+\.[0-9]+\.[0-9]+' \
        | sort -rV \
        | head -n1; } || true
}

# The [package] version of a Cargo.toml on stdin.
node_version_from_cargo() {
    { sed -n '/^\[package\]/,/^\[/{ /^version[[:space:]]*=/{ s/^[^"]*"\([^"]*\)".*/\1/p; q; } }' \
        | grep -E '^[0-9]+\.[0-9]+\.[0-9]+$' \
        | head -n1; } || true
}

# Find the latest node version into LATEST_NODE (empty if every route failed).
# Trusted sources only (a public accelerator could name any version):
#   1. GitHub API, direct
#   2. GitHub API through the trusted relay
#   3. Cargo.toml on main (raw.githubusercontent.com), direct then the trusted relay
resolve_node_version() {
    local api="https://api.github.com/repos/${REPO}/releases?per_page=30"
    local raw="https://raw.githubusercontent.com/${REPO}/main/crates/node/Cargo.toml"
    local i label
    LATEST_NODE="$(curl_small_text "$api" | node_version_from_releases)"
    if [ -n "$LATEST_NODE" ]; then
        info "Version lookup: GitHub API (direct)"
        return 0
    fi
    warn "Version lookup: GitHub API (direct) failed; trying mirror"
    LATEST_NODE="$(curl_small_text "${GH_PREFIXES[1]}${api}" | node_version_from_releases)"
    if [ -n "$LATEST_NODE" ]; then
        info "Version lookup: GitHub API via mirror"
        return 0
    fi
    warn "Version lookup: GitHub API via mirror failed; reading Cargo.toml from the repo"
    for ((i = 0; i < TRUSTED_MIRRORS; i++)); do
        label="$(mirror_label "$i")"
        LATEST_NODE="$(curl_small_text "${GH_PREFIXES[$i]}${raw}" | node_version_from_cargo)"
        if [ -n "$LATEST_NODE" ]; then
            info "Version lookup: repository Cargo.toml via ${label}"
            return 0
        fi
        warn "Version lookup: Cargo.toml via ${label} failed"
    done
    return 0
}

# Download the program: RELAY_NODE_BASE_URL (your own mirror) alone if set,
# otherwise GitHub through the source list.
download_binary() {
    # $1 = GitHub URL, $2 = custom mirror URL or "", $3 = destination file
    if [ -n "$2" ]; then
        curl "${CURL_OPTS[@]}" "$2" -o "$3"
    else
        fetch_with_mirrors "relay-node" "$1" "$3" large valid_program_file
    fi
}

# Download the program's .sha256: from TRUSTED sources only (direct GitHub, then
# the trusted relay). A custom mirror (RELAY_NODE_BASE_URL) may not serve one;
# the caller treats that as a warning, not an error.
fetch_checksum() {
    # $1 = GitHub .sha256 URL, $2 = custom mirror .sha256 URL or "", $3 = destination file
    if [ -n "$2" ]; then
        curl -fsSL --connect-timeout 10 --max-time 30 --retry 2 --retry-delay 2 \
            --retry-connrefused ${PROXY:+--proxy "$PROXY"} "$2" -o "$3"
    else
        fetch_with_mirrors "sha256" "$1" "$3" small valid_sha256_file
    fi
}

# ---------- Resolve the install version ----------
# v1.2: nodes release on their own node-v* track. By default install the LATEST
# node-v* tag from GitHub; --version pins a specific one. The script NEVER
# falls back to the panel version — if every version lookup fails it uses
# SCRIPT_VERSION (the version this script shipped with) and warns.
if [ -z "$TARGET_VERSION" ]; then
    info "Looking up the latest node-v* release…"
    resolve_node_version
    if [ -n "$LATEST_NODE" ]; then
        TARGET_VERSION="$LATEST_NODE"
        info "Latest node release: $TARGET_VERSION"
    else
        # Query failed — do NOT guess the panel version. Fall back to the
        # version this script shipped with and warn.
        TARGET_VERSION="$SCRIPT_VERSION"
        warn "Could not reach GitHub (directly or through any accelerator) to find the latest node-v* release; falling back to bundled version $SCRIPT_VERSION. Use --version X.Y.Z to pin a specific version."
    fi
else
    info "Installing node version: $TARGET_VERSION (pinned via --version)"
fi

# ---------- Already-latest check ----------
# v1.2: if the installed binary already reports this version, do NOT re-download
# or replace the binary (avoids a needless stop/swap/restart of a working node).
# BUT the operator may be re-running the installer to point the node at a NEW
# panel URL / token or to refresh the systemd unit — so we still rewrite
# start.sh, the env file, and the service file, then restart, below. The binary
# download/swap is the only step skipped (gated by ALREADY_AT_VERSION).
ALREADY_AT_VERSION=0
if [ -x "$BINARY" ]; then
    INSTALLED_VERSION="$("$BINARY" --version 2>/dev/null | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -n1 || true)"
    if [ -n "$INSTALLED_VERSION" ] && [ "$INSTALLED_VERSION" = "$TARGET_VERSION" ]; then
        info "Already at node version $TARGET_VERSION — skipping binary download; refreshing panel/token/service config."
        ALREADY_AT_VERSION=1
    else
        [ -n "$INSTALLED_VERSION" ] && info "Installed version: $INSTALLED_VERSION → upgrading to $TARGET_VERSION"
    fi
fi

# v1.2: skip the binary download/verify/swap entirely when already at the
# target version (ALREADY_AT_VERSION=1). The config/service rewrite below still
# runs so a re-invocation with a new -t/-u refreshes the node's panel binding.
if [ "$ALREADY_AT_VERSION" != "1" ]; then

# ---------- Build download URL ----------
# v1.2: node binaries live under node-v{version} from 1.1.1 onward. Versions
# 1.1.0 and earlier were published under the joint v{version} tag, so fall back
# to that path for the known legacy range (mirrors the node's self-updater).
# Default: GitHub Releases (with the accelerators above). Override with
# RELAY_NODE_BASE_URL for a mirror of your own, which is then used alone.
ASSET_NAME="relay-node-linux-${ARCH}"
url_tag_prefix() {
    # $1 = bare version. Echo "node-v" or "v" (legacy fallback for <= 1.1.0).
    local v="$1"
    # Compare major.minor.patch numerically against 1.1.1.
    local major minor patch
    IFS=. read -r major minor patch <<< "${v%%-*}"
    major="${major:-0}"; minor="${minor:-0}"; patch="${patch:-0}"
    if [ "$major" -lt 1 ] \
        || { [ "$major" -eq 1 ] && [ "$minor" -lt 1 ]; } \
        || { [ "$major" -eq 1 ] && [ "$minor" -eq 1 ] && [ "$patch" -lt 1 ]; }; then
        echo "v"
    else
        echo "node-v"
    fi
}
GITHUB_URL=""
CUSTOM_URL=""
if [ -n "${RELAY_NODE_BASE_URL:-}" ]; then
    CUSTOM_URL="${RELAY_NODE_BASE_URL}/${ASSET_NAME}"
    DOWNLOAD_URL="$CUSTOM_URL"
    info "Using custom mirror: ${RELAY_NODE_BASE_URL}"
else
    TAG_PREFIX=$(url_tag_prefix "$TARGET_VERSION")
    GITHUB_URL="https://github.com/${REPO}/releases/download/${TAG_PREFIX}${TARGET_VERSION}/${ASSET_NAME}"
    DOWNLOAD_URL="$GITHUB_URL"
    info "Download URL: $DOWNLOAD_URL (direct first, then accelerators)"
fi

# ---------- Download binary to temp file ----------
# curl flags for a custom mirror (RELAY_NODE_BASE_URL): follow redirects, fail on
# HTTP error, show progress bar, connect timeout 10s, total timeout 120s, retry
# 3x with 2s delay. GitHub downloads use fetch_with_mirrors instead.
CURL_OPTS=(-fL --progress-bar --connect-timeout 10 --max-time 120 --retry 3 --retry-delay 2 --retry-connrefused)
if [ -n "$PROXY" ]; then
    info "Using proxy: $PROXY"
    CURL_OPTS+=(--proxy "$PROXY")
fi

info "Downloading relay-node v${TARGET_VERSION} (${ARCH}) ..."
info "  URL: $DOWNLOAD_URL"
mkdir -p "$INSTALL_DIR"

# Always start from a clean temp file so a previous failed run cannot leave
# a half-written file that passes the ELF check by accident.
rm -f "$TMP_BINARY"

# GitHub path: get the sha256 FIRST, from GitHub itself (direct or
# the trusted relay, never a third-party accelerator). Without it nothing is
# downloaded: a program from a public accelerator could not be checked. Fails
# closed; the only way past is the explicit SKIP_CHECKSUM=1, in which case the
# program is taken from GitHub itself only.
TMP_CHECKSUM="${TMP_BINARY}.sha256"
rm -f "$TMP_CHECKSUM"
EXPECTED_SHA256=""
if [ -z "$CUSTOM_URL" ] && [ "${SKIP_CHECKSUM:-0}" != "1" ]; then
    if ! fetch_checksum "${GITHUB_URL}.sha256" "" "$TMP_CHECKSUM"; then
        rm -f "$TMP_CHECKSUM"
        fail "Could not get the sha256 for release v${TARGET_VERSION} from GitHub itself
(tried: $(mirror_label 0), $(mirror_label 1)). Nothing was downloaded or installed,
because a program cannot be verified without it.

Try one of:
  1. Run the installer again in a few minutes
  2. Use a proxy:     $0 -t <token> -u <url> -p socks5://127.0.0.1:10808
  3. Use a mirror:    RELAY_NODE_BASE_URL=https://your-mirror.com $0 -t <token> -u <url>
  4. Skip the check (NOT recommended; the program then comes from GitHub itself only):
     SKIP_CHECKSUM=1 $0 -t <token> -u <url>

The existing binary (if any) was NOT touched."
    fi
    EXPECTED_SHA256="$(awk '{ print $1; exit }' "$TMP_CHECKSUM" | tr -d '[:space:]' | tr '[:upper:]' '[:lower:]')"
fi

if ! download_binary "$GITHUB_URL" "$CUSTOM_URL" "$TMP_BINARY"; then
    rm -f "$TMP_BINARY"
    echo ""
    fail "Download failed. Possible causes:
  - GitHub Releases and every accelerator (mirror, gh-proxy.com,
    ghfast.top) are blocked, unreachable or too slow from this network
  - Release v${TARGET_VERSION} does not have asset ${ASSET_NAME}
  - Every copy offered did not match GitHub's sha256 (see the messages above)
  - Proxy is misconfigured (if you passed -p)

Try one of:
  1. Use a proxy:     $0 -t <token> -u <url> -p socks5://127.0.0.1:10808
  2. Use a mirror:    RELAY_NODE_BASE_URL=https://your-mirror.com $0 -t <token> -u <url>
  3. Download manually:
     curl -fL -o relay-node '${DOWNLOAD_URL}'
     Then copy to ${BINARY}

Note: the existing binary (if any) was NOT touched by this failed download."
fi

# ---------- Verify SHA256 checksum (supply-chain integrity) ----------
# v0.3.9: every GitHub Release publishes <asset>.sha256 alongside the binary.
# We download it and run `sha256sum -c` so a tampered/truncated/replaced binary
# (MITM, compromised mirror, CDN cache poisoning) is caught BEFORE it replaces
# the working binary. The existing binary is NOT touched on verification
# failure (same guarantee as a failed download).
#
# Behavior:
#   - GitHub Releases (default): checksum is REQUIRED. Missing file or mismatch
#     is a hard FAIL — the release is expected to ship one (binary-release.yml
#     generates it). Set SKIP_CHECKSUM=1 only if you accept the risk.
#   - Custom mirror (RELAY_NODE_BASE_URL): we try the checksum but, if the
#     mirror doesn't serve a .sha256, we WARN and continue (a mirror operator
#     may legitimately not mirror it). A mismatch is still a hard FAIL.
if [ "${SKIP_CHECKSUM:-0}" != "1" ]; then
    CHECKSUM_URL="${DOWNLOAD_URL}.sha256"
    # v0.3.11: MUST use -L (follow redirects). GitHub Releases download URLs
    # 302-redirect to objects.githubusercontent.com; without -L, curl returns
    # exit 0 but downloads an EMPTY body (the 3xx response), so the checksum
    # file ends up 0 bytes and we falsely report "empty or malformed". Reuse
    # the same redirect/retry/proxy flags as the binary download for parity.
    # On GitHub the .sha256 was already fetched above, from GitHub itself only
    # (direct or the trusted relay), before the program was downloaded; the
    # comparison below is the final gate. A custom mirror's checksum is fetched
    # here, from the mirror only.
    if [ -z "$CUSTOM_URL" ] || fetch_checksum "" "${CUSTOM_URL}.sha256" "$TMP_CHECKSUM"; then
        # v0.3.10: verify by DIRECT hash comparison rather than `sha256sum -c`.
        # The `-c` mode re-parses the filename from the checksum file, whose
        # behaviour varies across GNU coreutils / BusyBox / toybox (filename
        # quoting, leading `*` for binary mode, path handling) and produced
        # false FAILs on otherwise-correct downloads. Extracting the hash and
        # comparing it as a plain string is robust across all of them.
        EXPECTED=$(awk '{ print $1 }' "$TMP_CHECKSUM" | tr -d '[:space:]')
        # Compute the actual hash, trying the common tool variants in order.
        #   sha256sum <file> | awk '{print $1}'   (GNU coreutils, BusyBox)
        #   shasum -a 256 <file> | awk '{print $1}' (macOS, some BSD-based)
        #   sha256 <file>   (Alpine/FreeBSD: prints just the hash)
        ACTUAL=""
        if command -v sha256sum >/dev/null 2>&1; then
            ACTUAL=$(sha256sum "$TMP_BINARY" 2>/dev/null | awk '{ print $1 }')
        elif command -v shasum >/dev/null 2>&1; then
            ACTUAL=$(shasum -a 256 "$TMP_BINARY" 2>/dev/null | awk '{ print $1 }')
        elif command -v sha256 >/dev/null 2>&1; then
            ACTUAL=$(sha256 "$TMP_BINARY" 2>/dev/null | awk '{ print $1 }')
        fi
        # Normalize both to lowercase hex so a case difference never causes a
        # false mismatch (some tools uppercase).
        EXPECTED_LC=$(printf '%s' "$EXPECTED" | tr '[:upper:]' '[:lower:]')
        ACTUAL_LC=$(printf '%s' "$ACTUAL" | tr '[:upper:]' '[:lower:]')
        if [ -z "$EXPECTED" ]; then
            rm -f "$TMP_BINARY" "$TMP_CHECKSUM"
            fail "Checksum file at ${CHECKSUM_URL} is empty or malformed.
The download was discarded. To bypass (NOT recommended): SKIP_CHECKSUM=1"
        elif [ -z "$ACTUAL_LC" ]; then
            rm -f "$TMP_BINARY" "$TMP_CHECKSUM"
            fail "No sha256 tool found (tried sha256sum, shasum, sha256).
Cannot verify the downloaded binary. Install one of these, or re-run with
SKIP_CHECKSUM=1 (NOT recommended). The existing binary was NOT touched."
        elif [ "$ACTUAL_LC" = "$EXPECTED_LC" ]; then
            info "Checksum verified (sha256 OK)."
        else
            rm -f "$TMP_BINARY" "$TMP_CHECKSUM"
            fail "Checksum verification FAILED for ${ASSET_NAME}.
Expected: $EXPECTED
Actual:   $ACTUAL

The downloaded binary does not match the published sha256. This indicates a
truncated, corrupted, or tampered download. The existing binary was NOT touched.
To bypass (NOT recommended), re-run with SKIP_CHECKSUM=1."
        fi
        rm -f "$TMP_CHECKSUM"
    elif [ -z "${RELAY_NODE_BASE_URL:-}" ]; then
        # GitHub Releases is REQUIRED to ship a checksum. Missing = hard fail.
        rm -f "$TMP_BINARY" "$TMP_CHECKSUM"
        fail "Checksum file not found at ${CHECKSUM_URL}.
Release v${TARGET_VERSION} is expected to publish a .sha256. The download was
discarded. To bypass (NOT recommended): SKIP_CHECKSUM=1"
    else
        warn "No checksum available at mirror (${CHECKSUM_URL}); skipping verification.
Prefer a mirror that also serves .sha256, or set SKIP_CHECKSUM=1 to silence this."
    fi
fi

# ---------- Validate downloaded temp file ----------
# Check it is not an HTML error page. Use two methods:
#   1. Read the first 4 bytes - ELF binaries start with 0x7f 'E' 'L' 'F'
#   2. If `file` command exists, use it as a secondary check
# This avoids depending on `file` being installed (many minimal images lack it).
ELF_MAGIC=$(head -c 4 "$TMP_BINARY" 2>/dev/null | xxd -p 2>/dev/null || od -A n -t x1 -N 4 "$TMP_BINARY" 2>/dev/null | tr -d ' \n')

if [ "$ELF_MAGIC" != "7f454c46" ]; then
    # Not an ELF - try `file` for a better error message
    FILE_DESC=$(file -b "$TMP_BINARY" 2>/dev/null || echo "not an ELF binary")
    rm -f "$TMP_BINARY"
    fail "Downloaded file is not a valid binary (${FILE_DESC}).
The download URL may have returned an error page. Check:
  ${DOWNLOAD_URL}

Note: the existing binary (if any) was NOT touched."
fi

FILE_SIZE=$(stat -c%s "$TMP_BINARY" 2>/dev/null || stat -f%z "$TMP_BINARY" 2>/dev/null || echo 0)
if [ "$FILE_SIZE" -lt 100000 ]; then
    rm -f "$TMP_BINARY"
    fail "Downloaded file is too small (${FILE_SIZE} bytes). Expected a multi-MB binary.

Note: the existing binary (if any) was NOT touched."
fi

# ---------- Atomic install: stop service, swap binary ----------
# We stop the service ONLY after the new binary is fully downloaded and
# validated. This guarantees a failed download or validation never leaves
# the node without a working binary.
#
# Why stop at all: writing over a running ELF fails with ETXTBSY
# ("Text file busy"). The download went to .tmp so we avoided that during
# curl, but we still stop before the swap so the old binary releases cleanly
# and the new one starts from a known state.
if systemctl list-unit-files 2>/dev/null | grep -q "^${SERVICE_NAME}\.service"; then
    if systemctl is-active --quiet "$SERVICE_NAME"; then
        info "Stopping existing ${SERVICE_NAME} service for binary swap ..."
        systemctl stop "$SERVICE_NAME" || warn "systemctl stop returned non-zero (continuing)"
    else
        info "Existing ${SERVICE_NAME} service detected but not running."
    fi
else
    info "No existing ${SERVICE_NAME} service detected (fresh install)."
fi

# mv -f on the same filesystem is a single rename(2) syscall: atomic and
# never partial. The old running binary (if any) keeps its inode alive for
# the already-open file descriptors, so this cannot corrupt anything.
mv -f "$TMP_BINARY" "$BINARY"
chmod +x "$BINARY"
info "Binary installed: ${BINARY} ($(( FILE_SIZE / 1024 / 1024 )) MB, ELF ${ARCH})"

fi  # end ALREADY_AT_VERSION skip (binary download/swap)

# ---------- Write start.sh ----------
# v1.2.12: start.sh works from ITS OWN install dir. It used to cd into (and
# source relay-node.env from) a hardcoded /opt/relay-node, so an instance
# installed with -s ran from the default instance's directory.
START_SH="${INSTALL_DIR}/start.sh"
info "Writing start script: $START_SH"
write_start_sh "$START_SH" "$INSTALL_DIR" "$PANEL_URL" "$NODE_TOKEN"
chmod 700 "$START_SH"

# Safety check: make sure /dev/fd did not leak into the file.
if grep -q '/dev/fd' "$START_SH" 2>/dev/null; then
    fail "start.sh generated incorrectly (contains /dev/fd). Aborting."
fi

# Write an example env file if one doesn't exist (don't overwrite an existing
# one — the operator may have configured it).
ENV_FILE="${INSTALL_DIR}/relay-node.env"
if [ ! -f "$ENV_FILE" ]; then
    info "Writing example env file: $ENV_FILE (edit to tune listen / egress / shutdown)"
    cat > "$ENV_FILE" <<'ENVEOF'
# relay-node optional environment. start.sh sources this file (set -a), so any
# variable set here is exported to relay-node. All values below are commented:
# the defaults already enable IPv4+IPv6 listening with system-routed egress.

# ── v1.0.5: dual-stack inbound listen (TCP/UDP) ──
# By default every rule listens on BOTH 0.0.0.0 and :: (separate sockets, the
# IPv6 one is IPV6_V6ONLY). Set a family's variable EMPTY to disable it.
#   LISTEN_IPV4=0.0.0.0      # empty → IPv4 disabled (IPv6-only node)
#   LISTEN_IPV6=::           # empty → IPv6 disabled (IPv4-only node)

# ── v1.0.5: IPv4 outbound egress for multi-NIC servers ──
# When inbound (e.g. public IPv6 on ens19) and outbound (e.g. IPv4 via ens18)
# use different interfaces, force the source so connections leave the right NIC.
# Priority: OUTBOUND_BIND_IPV4 > OUTBOUND_INTERFACE > auto (system routing).
# The example values below are ILLUSTRATIVE — use your own server's NIC/address.
#   OUTBOUND_INTERFACE=ens18         # node resolves this NIC's IPv4 to bind
#   OUTBOUND_BIND_IPV4=10.0.2.61     # or pin the exact source IPv4 (overrides above)

# ── v1.2.1: public-IP detection endpoints ──
# The node does NOT read its own NIC: it asks an external service what source
# address the outside sees, which is what makes a NAT'd host (private NIC IP,
# mapped elastic IP) report the right address. The answer becomes the IP and
# country flag shown on the panel; failure shows "-" and never affects
# forwarding. Re-checked every 30 minutes, so restart to apply a change.
#
# Override only if the defaults are unreachable from this server. Whatever you
# point these at MUST return a BARE IP and MUST be family-pinned — the node
# discards an answer from the wrong family, so a dual-stack endpoint (one that
# replies with whichever family you connected over) leaves the address blank on
# dual-stack hosts. Test with: curl -s --max-time 5 <url>
#   PUBLIC_IPV4_CHECK_URL=https://ipv4.icanhazip.com
#   PUBLIC_IPV6_CHECK_URL=https://ipv6.icanhazip.com

# ── v1.2.5: graceful shutdown ──
# On `systemctl stop` / `restart` or a one-click upgrade, the node stops
# accepting, reports the traffic counted so far, then waits up to this many
# seconds for open TCP connections to finish before exiting (it exits at once
# when none are open). New connections are refused during the wait, so keep it
# short. Long-lived connections (tunnels, VPNs) are still cut when it ends.
# 0 = no wait (the traffic report still runs). Default 5, maximum 60.
#   SHUTDOWN_DRAIN_SECS=5

# ── v1.2.6: UDP session limit ──
# Every UDP client address holds one socket until it has been idle for 60 s.
# New sessions beyond this many (all rules together) are refused, so a flood of
# forged source addresses cannot use up the node's file descriptors and stop
# every rule. Default: half the file-descriptor limit (32768 with this
# service's LimitNOFILE=65536). Raise both for very busy UDP forwarding.
#   UDP_MAX_SESSIONS=32768
ENVEOF
    chmod 600 "$ENV_FILE"
fi

# ---------- Write systemd service ----------
SERVICE_FILE="/etc/systemd/system/${SERVICE_NAME}.service"
info "Writing systemd service: $SERVICE_FILE"
cat > "$SERVICE_FILE" <<SVCEOF
[Unit]
Description=RelayNode forwarding service
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
WorkingDirectory=${INSTALL_DIR}
ExecStart=/bin/bash ${START_SH}
Restart=always
RestartSec=3
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
SVCEOF

# ---------- Enable and start ----------
info "Enabling and starting ${SERVICE_NAME} ..."
systemctl daemon-reload
systemctl enable "$SERVICE_NAME"
# restart handles both fresh start and post-upgrade start.
systemctl restart "$SERVICE_NAME"

# ---------- Verify ----------
sleep 2
if systemctl is-active --quiet "$SERVICE_NAME"; then
    info "Service ${SERVICE_NAME} is running."
else
    warn "Service ${SERVICE_NAME} failed to start. Recent logs:"
    echo "---"
    journalctl -u "$SERVICE_NAME" --no-pager -n 50 2>/dev/null || echo "(journalctl not available)"
    echo "---"
fi

# ---------- Done ----------
echo ""
info "=========================================="
info " relay-node installed successfully!"
info "=========================================="
echo ""
echo "  Service:   ${SERVICE_NAME}"
echo "  Binary:    ${BINARY}"
echo "  Version:   v${TARGET_VERSION}"
echo "  Panel:     ${PANEL_URL}"
echo ""
echo "  Logs:      journalctl -u ${SERVICE_NAME} -f"
echo "  Status:    systemctl status ${SERVICE_NAME}"
echo "  Stop:      systemctl stop ${SERVICE_NAME}"
echo "  Restart:   systemctl restart ${SERVICE_NAME}"
echo "  Upgrade:   re-run this installer with the same -t/-u flags"
echo "  Uninstall: systemctl disable --now ${SERVICE_NAME}; rm -f ${SERVICE_FILE}; rm -rf ${INSTALL_DIR}"
echo ""
