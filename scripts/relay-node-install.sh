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
#
# Idempotent re-runs: downloading to a temp file and swapping atomically means
# this script can be re-run to upgrade an already-running node. The running
# binary is only replaced AFTER the new one is fully downloaded and validated,
# so a failed download never breaks an existing install.
#
set -euo pipefail

# Bump this when releasing a new version. The binary is downloaded from
# GitHub Releases assets.
SCRIPT_VERSION="1.0.4"
REPO="MoeShinX/relay-panel"

GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m'

info()  { echo -e "${GREEN}[INFO]${NC}  $*"; }
warn()  { echo -e "${YELLOW}[WARN]${NC}  $*"; }
fail()  { echo -e "${RED}[FAIL]${NC}  $*"; exit 1; }

# ---------- Defaults ----------
NODE_TOKEN=""
PANEL_URL=""
SERVICE_NAME="relay-node"
PROXY="${RELAY_PROXY:-}"

# ---------- Parse args ----------
while [[ $# -gt 0 ]]; do
    case "$1" in
        -t|--token)         NODE_TOKEN="$2"; shift 2 ;;
        -u|--url)           PANEL_URL="$2"; shift 2 ;;
        -s|--service-name)  SERVICE_NAME="$2"; shift 2 ;;
        -p|--proxy)         PROXY="$2"; shift 2 ;;
        -h|--help)
            echo "Usage: $0 -t <token> -u <panel-url> [-s <service-name>] [-p <proxy>]"
            echo ""
            echo "Options:"
            echo "  -t, --token         Node token from the panel UI (required)"
            echo "  -u, --url           Panel URL, e.g. http://panel-ip:18888 (required)"
            echo "  -s, --service-name  systemd service name (default: relay-node)"
            echo "  -p, --proxy         Download proxy, e.g. socks5://127.0.0.1:10808"
            echo ""
            echo "Environment:"
            echo "  RELAY_PROXY           Same as -p"
            echo "  RELAY_NODE_BASE_URL   Custom mirror for binary downloads"
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
    fail "Missing required option: -u/--url. Example: http://45.149.92.10:18888"
fi

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

# ---------- Build download URL ----------
# Default: GitHub Releases. Override with RELAY_NODE_BASE_URL for mirrors.
ASSET_NAME="relay-node-linux-${ARCH}"
if [ -n "${RELAY_NODE_BASE_URL:-}" ]; then
    DOWNLOAD_URL="${RELAY_NODE_BASE_URL}/${ASSET_NAME}"
    info "Using custom mirror: ${RELAY_NODE_BASE_URL}"
else
    DOWNLOAD_URL="https://github.com/${REPO}/releases/download/v${SCRIPT_VERSION}/${ASSET_NAME}"
fi

# ---------- Download binary to temp file ----------
# curl flags: follow redirects, fail on HTTP error, show progress bar,
# connect timeout 10s, total timeout 120s, retry 3x with 2s delay.
CURL_OPTS=(-fL --progress-bar --connect-timeout 10 --max-time 120 --retry 3 --retry-delay 2 --retry-connrefused)
if [ -n "$PROXY" ]; then
    info "Using proxy: $PROXY"
    CURL_OPTS+=(--proxy "$PROXY")
fi

info "Downloading relay-node v${SCRIPT_VERSION} (${ARCH}) ..."
info "  URL: $DOWNLOAD_URL"
mkdir -p "$INSTALL_DIR"

# Always start from a clean temp file so a previous failed run cannot leave
# a half-written file that passes the ELF check by accident.
rm -f "$TMP_BINARY"

if ! curl "${CURL_OPTS[@]}" "$DOWNLOAD_URL" -o "$TMP_BINARY"; then
    rm -f "$TMP_BINARY"
    echo ""
    fail "Download failed. Possible causes:
  - GitHub Releases is blocked or slow in your network
  - Release v${SCRIPT_VERSION} does not have asset ${ASSET_NAME}
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
    TMP_CHECKSUM="${TMP_BINARY}.sha256"
    # v0.3.11: MUST use -L (follow redirects). GitHub Releases download URLs
    # 302-redirect to objects.githubusercontent.com; without -L, curl returns
    # exit 0 but downloads an EMPTY body (the 3xx response), so the checksum
    # file ends up 0 bytes and we falsely report "empty or malformed". Reuse
    # the same redirect/retry/proxy flags as the binary download for parity.
    if curl -fsSL --connect-timeout 10 --max-time 30 --retry 2 --retry-delay 2 \
        --retry-connrefused ${PROXY:+--proxy "$PROXY"} "$CHECKSUM_URL" -o "$TMP_CHECKSUM"; then
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
Release v${SCRIPT_VERSION} is expected to publish a .sha256. The download was
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

# ---------- Write start.sh ----------
# IMPORTANT: the here-doc uses quoted 'EOF' delimiter so NOTHING is expanded
# at install time - all values are written literally. This prevents bugs like
# $0 being /dev/fd/63 when the installer runs via bash <(curl ...).
START_SH="${INSTALL_DIR}/start.sh"
info "Writing start script: $START_SH"
cat > "$START_SH" <<'STARTEOF'
#!/usr/bin/env bash
set -euo pipefail
cd "/opt/relay-node"
export PANEL_URL="__PANEL_URL__"
export NODE_TOKEN="__NODE_TOKEN__"
export POLL_INTERVAL="${POLL_INTERVAL:-10}"
export RUST_LOG="${RUST_LOG:-info}"
# Optional config sourced from relay-node.env if present (written by the
# installer with commented examples; edit it to set LISTEN_IPV4/LISTEN_IPV6,
# OUTBOUND_INTERFACE/OUTBOUND_BIND_IPV4, or TLS_CERT_PATH/TLS_KEY_PATH). If the
# file doesn't exist, all of these stay unset and the node uses its defaults
# (dual-stack listen, system-routed egress, no TLS).
# NOTE: path is hardcoded (/opt/relay-node) because this script runs with set -u
# and INSTALL_DIR is not defined in the generated start.sh context.
if [ -f "/opt/relay-node/relay-node.env" ]; then
    set -a
    . "/opt/relay-node/relay-node.env"
    set +a
fi
exec ./relay-node
STARTEOF

# Replace the placeholders with actual values (safe - no shell expansion).
sed -i "s|__PANEL_URL__|${PANEL_URL}|" "$START_SH"
sed -i "s|__NODE_TOKEN__|${NODE_TOKEN}|" "$START_SH"
chmod 700 "$START_SH"

# Safety check: make sure /dev/fd did not leak into the file.
if grep -q '/dev/fd' "$START_SH" 2>/dev/null; then
    fail "start.sh generated incorrectly (contains /dev/fd). Aborting."
fi

# v0.4.1: create the certs directory + example env file for TLS Simple.
# The operator places their cert+key here (or points the env file elsewhere).
CERTS_DIR="${INSTALL_DIR}/certs"
mkdir -p "$CERTS_DIR"
chmod 700 "$CERTS_DIR"

# Write an example env file if one doesn't exist (don't overwrite an existing
# one — the operator may have configured it).
ENV_FILE="${INSTALL_DIR}/relay-node.env"
if [ ! -f "$ENV_FILE" ]; then
    info "Writing example env file: $ENV_FILE (edit to tune listen / egress / TLS)"
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

# ── TLS Simple certificate configuration (v0.4.1) ──
# Uncomment and set these to enable TLS Simple ingress on this node.
# The cert must be PEM format (fullchain recommended); the key must be PEM
# (PKCS#8, PKCS#1 RSA, or SEC1 EC). The key file MUST be chmod 600.
#   TLS_CERT_PATH=/opt/relay-node/certs/fullchain.pem
#   TLS_KEY_PATH=/opt/relay-node/certs/privkey.pem
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
echo "  Version:   v${SCRIPT_VERSION}"
echo "  Panel:     ${PANEL_URL}"
echo ""
echo "  Logs:      journalctl -u ${SERVICE_NAME} -f"
echo "  Status:    systemctl status ${SERVICE_NAME}"
echo "  Stop:      systemctl stop ${SERVICE_NAME}"
echo "  Restart:   systemctl restart ${SERVICE_NAME}"
echo "  Upgrade:   re-run this installer with the same -t/-u flags"
echo "  Uninstall: systemctl disable --now ${SERVICE_NAME}; rm -f ${SERVICE_FILE}; rm -rf ${INSTALL_DIR}"
echo ""
