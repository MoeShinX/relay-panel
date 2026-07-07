#!/usr/bin/env bash
#
# Pre-release consistency check.
#
# Run BEFORE tagging a new release. Verifies that every place a version number
# lives in the repo agrees with the version you pass on the command line.
# Does NOT modify any file. Exits 1 on FAIL (so it can plug into CI later).
#
# v1.2: panel and node release on INDEPENDENT tracks. Pass a `panel` or `node`
# subcommand to check ONLY that track's version locations:
#   bash scripts/release-check.sh panel 1.1.1   # panel release (tag v1.1.1)
#   bash scripts/release-check.sh node 1.1.0    # node release (tag node-v1.1.0)
# For backwards compatibility, a bare version (no subcommand) defaults to panel:
#   bash scripts/release-check.sh 1.1.1         # == panel 1.1.1
#
# Exit codes:
#   0  - all checks pass (warnings allowed)
#   1  - usage error OR at least one FAIL
#
# The full checklist this script enforces is documented in docs/VERSIONS.md.
#

set -euo pipefail

# On Windows Git Bash (MSYS2), unquoted values that look like paths (e.g.
# "0.2.1" or grep patterns) get mangled by automatic path conversion, which
# breaks the version greps. Disable it. Harmless / ignored on Linux & macOS.
export MSYS_NO_PATHCONV=1

# ---------- Counters ----------
OK=0
WARN=0
FAIL=0

# ---------- Pretty output (no color if not a TTY) ----------
if [ -t 1 ]; then
    C_OK="\033[0;32m"
    C_WARN="\033[0;33m"
    C_FAIL="\033[0;31m"
    C_RESET="\033[0m"
else
    C_OK=""; C_WARN=""; C_FAIL=""; C_RESET=""
fi

ok()   { echo "  ${C_OK}[OK]${C_RESET}   $1"; OK=$((OK+1)); }
warn() { echo "  ${C_WARN}[WARN]${C_RESET} $1"; WARN=$((WARN+1)); }
fail() { echo "  ${C_FAIL}[FAIL]${C_RESET} $1"; FAIL=$((FAIL+1)); }
section() { echo ""; echo "== $1 =="; }

# ---------- Argument parsing ----------
# v1.2: optional first arg is the track: `panel` or `node`. A bare version (no
# recognized track keyword) defaults to panel for backwards compatibility.
TRACK="panel"
if [ $# -ge 2 ]; then
    case "$1" in
        panel|node) TRACK="$1"; shift ;;
        *) ;;  # fall through: $1 is a version, not a track keyword
    esac
fi
if [ $# -lt 1 ]; then
    echo "Usage: bash scripts/release-check.sh [panel|node] <version>"
    echo "  e.g. bash scripts/release-check.sh panel 1.1.1   # panel release (tag v1.1.1)"
    echo "       bash scripts/release-check.sh node 1.1.0    # node release (tag node-v1.1.0)"
    echo "       bash scripts/release-check.sh 1.1.1         # defaults to panel"
    exit 1
fi

RAW="$1"
# Normalize: strip leading 'v' or 'V' if present. (A node tag prefix 'node-v'
# is NOT passed here — the caller passes the bare version.)
VERSION="${RAW#[vV]}"
# A version must be 3 dot-separated numbers, optionally followed by a SemVer
# pre-release suffix (-alpha, -beta.1, -rc.2, …). We accept both stable
# (0.3.0) and pre-release (0.3.0-alpha) forms so the check works for alpha/beta
# cuts too. Build metadata (+build) is rejected to keep it simple.
if ! [[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?$ ]]; then
    echo "ERROR: '$RAW' is not a valid semver-like version (expected x.y.z, vx.y.z, x.y.z-suffix, or vx.y.z-suffix)"
    exit 1
fi
# v1.2: the tag prefix is track-specific. panel -> vX.Y.Z, node -> node-vX.Y.Z.
if [ "$TRACK" = "node" ]; then
    TAG="node-v${VERSION}"
else
    TAG="v${VERSION}"
fi

# Find the repo root (this script lives in <root>/scripts/).
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$ROOT"

echo "Release pre-flight check for ${TRACK} version: $VERSION (tag: $TAG)"
echo "Repo root: $ROOT"

# ---------- Helper: read a single value from a TOML file ----------
# Reads `key = "value"` or `key = value`. First match wins. Returns empty if
# not found. Pure grep/sed — no python / no awk script-blocks (Windows Git
# Bash can hit shell parsing issues on embedded awk regex bodies).
toml_value() {
    # $1 = file, $2 = key
    local file="$1" key="$2"
    [ -f "$file" ] || { echo ""; return 0; }
    # 1) find the first line that starts with `key =` (allow leading ws)
    # 2) split on first `=`; strip inline `# comment`, surrounding quotes, ws.
    # POSIX-portable sed/grep.
    grep -E "^[[:space:]]*${key}[[:space:]]*=" "$file" 2>/dev/null \
        | head -n1 \
        | sed -E 's/^[[:space:]]*[^=]+=[[:space:]]*//' \
        | sed -E 's/[[:space:]]*#.*$//' \
        | sed -E 's/^[[:space:]]*//; s/[[:space:]]*$//' \
        | sed -E 's/^"(.*)"$/\1/' || true
}

# ---------- Helper: read a value from rust source ----------
# Reads `const NAME: &str = "value";` (or `const NAME = "value";` without the
# type ascription). Used to grab COMPILED_APP_VERSION which is now wrapped in
# a function returning &'static str.
rust_const_value() {
    # $1 = file, $2 = constant name
    local file="$1" name="$2"
    [ -f "$file" ] || { echo ""; return 0; }
    # Match `const NAME<colon?><...>=` followed by a quoted string. The first
    # capture group is the value. POSIX ERE.
    grep -oE "const[[:space:]]+${name}[[:space:]]*(:[[:space:]]*&str)?[[:space:]]*=[[:space:]]*\"[^\"]+\"" "$file" 2>/dev/null \
        | head -n1 \
        | sed -E 's/.*"([^"]+)"$/\1/' || true
}

# ============================================================================
# 1. File existence
# ============================================================================
section "Required files"

REQUIRED_FILES=(
    "README.md"
    "README.en.md"
    "CHANGELOG.md"
    "CHANGELOG-NODE.md"
    "docs/DEPLOYMENT.md"
    "docs/REVERSE-PROXY.md"
    "docs/NODE.md"
    "docs/NODE.zh-CN.md"
    "docs/VERSIONS.md"
    "install.sh"
    "deploy.sh"
    "scripts/deploy-web-mode-check.sh"
    "scripts/relay-node-install.sh"
    "docker-compose.release.yaml"
    "crates/node/Cargo.toml"
    "crates/panel/Cargo.toml"
    "crates/shared/Cargo.toml"
    "crates/panel/src/config.rs"
    "Cargo.lock"
)
for f in "${REQUIRED_FILES[@]}"; do
    if [ -f "$f" ]; then
        ok "$f exists"
    else
        fail "$f missing"
    fi
done

# ============================================================================
# 2. Version string consistency
# v1.2: split by track. PANEL checks the panel-version locations; NODE checks
# the node-version locations. The two version sets are INDEPENDENT — a panel
# release must NOT require the node crate to match, and vice versa.
# ============================================================================
section "Version consistency (${TRACK})"

if [ "$TRACK" = "panel" ]; then
    # ── Panel track ──
    # crates/panel/src/config.rs COMPILED_APP_VERSION
    PANEL_VER=$(rust_const_value "crates/panel/src/config.rs" "COMPILED_APP_VERSION")
    if [ -z "$PANEL_VER" ]; then
        fail "crates/panel/src/config.rs: COMPILED_APP_VERSION not found"
    elif [ "$PANEL_VER" = "$VERSION" ]; then
        ok "crates/panel/src/config.rs COMPILED_APP_VERSION = $PANEL_VER"
    else
        fail "crates/panel/src/config.rs COMPILED_APP_VERSION = $PANEL_VER (expected $VERSION)"
    fi

    # docker-compose.release.yaml: PANEL image tag (node tag is independent).
    # v1.2: the tag may be a literal (:1.1.0) OR an env-var override with a
    # default (${RELAYPANEL_PANEL_TAG:-1.1.0}); match the version anywhere on
    # the panel image line.
    if grep -E "image:.*ghcr\.io/moeshinx/relay-panel-panel.*${VERSION}([^0-9.]|$)" docker-compose.release.yaml >/dev/null 2>&1; then
        ok "docker-compose.release.yaml panel image tag includes ${VERSION}"
    else
        fail "docker-compose.release.yaml: panel image tag ${VERSION} not found"
    fi

    # README dynamic release badge (reflects the latest GitHub panel release)
    for rf in README.md README.en.md; do
        if grep -q "github/v/release" "$rf" 2>/dev/null; then
            ok "$rf has dynamic release badge"
        else
            fail "$rf: dynamic release badge (github/v/release) not found"
        fi
    done

    # CHANGELOG.md panel section
    if grep -qE "^\#\#\s*\[${VERSION}\](\s*-|$)" CHANGELOG.md 2>/dev/null; then
        ok "CHANGELOG.md has section [${VERSION}]"
    else
        fail "CHANGELOG.md: no '## [${VERSION}]' section"
    fi
else
    # ── Node track ──
    # crates/node/Cargo.toml version (drives relay-node --version)
    NODE_VER=$(toml_value "crates/node/Cargo.toml" "version")
    if [ -z "$NODE_VER" ]; then
        fail "crates/node/Cargo.toml: no version found"
    elif [ "$NODE_VER" = "$VERSION" ]; then
        ok "crates/node/Cargo.toml version = $NODE_VER"
    else
        fail "crates/node/Cargo.toml version = $NODE_VER (expected $VERSION)"
    fi

    # scripts/relay-node-install.sh SCRIPT_VERSION (the version it installs)
    SCRIPT_VER=$(grep -E '^SCRIPT_VERSION=' scripts/relay-node-install.sh 2>/dev/null \
        | head -n1 \
        | sed -E 's/^SCRIPT_VERSION="([^"]+)"$/\1/' || true)
    if [ -z "$SCRIPT_VER" ]; then
        fail "scripts/relay-node-install.sh: SCRIPT_VERSION not found"
    elif [ "$SCRIPT_VER" = "$VERSION" ]; then
        ok "scripts/relay-node-install.sh SCRIPT_VERSION = $SCRIPT_VER"
    else
        fail "scripts/relay-node-install.sh SCRIPT_VERSION = $SCRIPT_VER (expected $VERSION)"
    fi

    # docker-compose.release.yaml: NODE image tag (panel tag is independent).
    # v1.2: the tag may be a literal (:1.1.0) OR an env-var override with a
    # default (${RELAYPANEL_NODE_TAG:-1.1.0}); match the version anywhere on
    # the node image line.
    if grep -E "image:.*ghcr\.io/moeshinx/relay-panel-node.*${VERSION}([^0-9.]|$)" docker-compose.release.yaml >/dev/null 2>&1; then
        ok "docker-compose.release.yaml node image tag includes ${VERSION}"
    else
        fail "docker-compose.release.yaml: node image tag ${VERSION} not found"
    fi

    # CHANGELOG-NODE.md node section (the node release body source)
    if grep -qE "^\#\#\s*\[${VERSION}\](\s*-|$)" CHANGELOG-NODE.md 2>/dev/null; then
        ok "CHANGELOG-NODE.md has section [${VERSION}]"
    else
        fail "CHANGELOG-NODE.md: no '## [${VERSION}]' section"
    fi
fi

# ============================================================================
# 3. Cargo supplementary checks (per-track)
# v1.2: only check THIS track's crate + Cargo.lock entry. The other track's
# crate version is intentionally independent and must NOT be forced to match.
# ============================================================================
section "Cargo supplementary (${TRACK})"

# 3.1 root Cargo.toml workspace.package.version (optional, shared)
ROOT_WS_VER=$(toml_value "Cargo.toml" "version")
if [ -n "$ROOT_WS_VER" ]; then
    if [ "$ROOT_WS_VER" = "$VERSION" ]; then
        ok "workspace.package.version = $ROOT_WS_VER"
    else
        warn "workspace.package.version = $ROOT_WS_VER (root workspace version is shared; not enforced per-track)"
    fi
else
    ok "no workspace.package.version in root Cargo.toml (skipped)"
fi

# 3.2 THIS track's crate Cargo.toml version — HARD FAIL on mismatch.
if [ "$TRACK" = "panel" ]; then
    CRATE_TOML="crates/panel/Cargo.toml"
    CRATE_NAME="relay-panel"
else
    CRATE_TOML="crates/node/Cargo.toml"
    CRATE_NAME="relay-node"
fi
CRATE_VER=$(toml_value "$CRATE_TOML" "version")
if [ -n "$CRATE_VER" ]; then
    if [ "$CRATE_VER" = "$VERSION" ]; then
        ok "${CRATE_TOML} version = $CRATE_VER"
    else
        fail "${CRATE_TOML} version = $CRATE_VER (expected $VERSION)"
    fi
else
    fail "${CRATE_TOML}: no version field (expected $VERSION)"
fi

# 3.3 crates/shared/Cargo.toml - WARN only (shared crate, not release-sync'd)
SHARED_TOML_VER=$(toml_value "crates/shared/Cargo.toml" "version")
if [ -n "$SHARED_TOML_VER" ]; then
    if [ "$SHARED_TOML_VER" = "$VERSION" ]; then
        ok "crates/shared/Cargo.toml version = $SHARED_TOML_VER"
    else
        warn "crates/shared/Cargo.toml version = $SHARED_TOML_VER (shared crate version is not used as release source yet.)"
    fi
else
    ok "crates/shared/Cargo.toml: no version field (skipped)"
fi

# 3.4 Cargo.lock: check ONLY this track's package + the shared crate. The OTHER
# track's package is intentionally at a different version and must NOT FAIL.
# shared is WARN only.
# Use awk for the small state machine (we keep this awk because it has no
# quoted regex body — just a single line, no `/.../` style patterns).
cargo_lock_version() {
    awk -v pkg="$1" '
        /^name = / {
            if ($0 == "name = \"" pkg "\"") { found=1; next }
        }
        found && /^version = / {
            s = $0
            i = index(s, "\"")
            if (i > 0) {
                j = index(substr(s, i + 1), "\"")
                if (j > 0) print substr(s, i + 1, j - 1)
            }
            found = 0
        }
    ' Cargo.lock 2>/dev/null || true
}

# This track's package: HARD FAIL on mismatch.
LOCK_VER=$(cargo_lock_version "$CRATE_NAME")
if [ -z "$LOCK_VER" ]; then
    warn "Cargo.lock: $CRATE_NAME not found (workspace may not include this crate?)"
elif [ "$LOCK_VER" = "$VERSION" ]; then
    ok "Cargo.lock $CRATE_NAME = $LOCK_VER"
else
    fail "Cargo.lock $CRATE_NAME = $LOCK_VER (expected $VERSION)"
fi

# shared crate: WARN only.
SHARED_LOCK_VER=$(cargo_lock_version "relay-shared")
if [ -n "$SHARED_LOCK_VER" ]; then
    if [ "$SHARED_LOCK_VER" = "$VERSION" ]; then
        ok "Cargo.lock relay-shared = $SHARED_LOCK_VER"
    else
        warn "Cargo.lock relay-shared = $SHARED_LOCK_VER (shared crate, not release-sync'd)"
    fi
fi

# 3.5 CHANGELOG section must be non-empty (per-track changelog file). A blank
# section would publish an empty GitHub Release body (the v0.3.4 body=null bug).
# The shared extractor exits non-zero on a missing / whitespace-only section.
if [ "$TRACK" = "panel" ]; then
    CHANGELOG_FILE="CHANGELOG.md"
else
    CHANGELOG_FILE="CHANGELOG-NODE.md"
fi
if bash scripts/extract-changelog.sh "$VERSION" "$CHANGELOG_FILE" >/dev/null 2>&1; then
    ok "${CHANGELOG_FILE} [${VERSION}] section is non-empty (release body will not be blank)"
else
    fail "${CHANGELOG_FILE} [${VERSION}] section is missing or empty (would publish an empty release body)"
fi

# ============================================================================
# 4. Script size + key content checks
# ============================================================================
section "Script size + content"

check_size_and_keys() {
    # $1 = file, $2 = min size, $3 = friendly name
    local file="$1" min="$2" name="$3"
    if [ ! -f "$file" ]; then
        fail "$name ($file) missing"
        return
    fi
    local size
    size=$(wc -c < "$file" | tr -d ' ')
    if [ "$size" -lt "$min" ]; then
        fail "$name ($file) is $size bytes (< $min) - looks empty or truncated"
    else
        ok "$name size = $size bytes (>= $min)"
    fi
}

check_size_and_keys "install.sh" 100 "install.sh"
check_size_and_keys "deploy.sh" 100 "deploy.sh"
check_size_and_keys "scripts/deploy-web-mode-check.sh" 100 "deploy web-mode harness"
check_size_and_keys "scripts/relay-node-install.sh" 100 "scripts/relay-node-install.sh"

# install.sh must mention key strings
check_in_file() {
    # $1 = file, $2 = substring, $3 = friendly description
    if grep -q -- "$2" "$1" 2>/dev/null; then
        ok "$3"
    else
        fail "$1 missing required content: $2 ($3)"
    fi
}

check_in_file "install.sh" "/opt/relay-panel" "install.sh mentions /opt/relay-panel"
check_in_file "install.sh" "git clone" "install.sh mentions 'git clone'"
check_in_file "install.sh" "./deploy.sh" "install.sh mentions './deploy.sh'"

check_in_file "deploy.sh" "docker-compose.release.yaml" "deploy.sh mentions docker-compose.release.yaml"
check_in_file "deploy.sh" "docker compose" "deploy.sh mentions 'docker compose'"
# GHCR or ghcr.io (case-insensitive)
if grep -qi -E "GHCR|ghcr\.io" deploy.sh 2>/dev/null; then
    ok "deploy.sh mentions GHCR / ghcr.io"
else
    fail "deploy.sh: no reference to GHCR / ghcr.io"
fi

check_in_file "scripts/deploy-web-mode-check.sh" "REVERSE_PROXY_EXTERNAL" "deploy web-mode harness covers separate-host reverse proxy"
check_in_file "scripts/deploy-web-mode-check.sh" "RELAYPANEL_WEB_MODE=caddy" "deploy web-mode harness covers Caddy mode"

check_in_file "scripts/relay-node-install.sh" "SCRIPT_VERSION" "install script has SCRIPT_VERSION"
check_in_file "scripts/relay-node-install.sh" "/opt/relay-node" "install script mentions /opt/relay-node"
check_in_file "scripts/relay-node-install.sh" "systemctl" "install script mentions systemctl"
check_in_file "scripts/relay-node-install.sh" 'relay-node-linux-${ARCH}' "install script uses relay-node-linux-\${ARCH} asset"
check_in_file "scripts/relay-node-install.sh" "NODE_TOKEN" "install script mentions NODE_TOKEN"
check_in_file "scripts/relay-node-install.sh" "PANEL_URL" "install script mentions PANEL_URL"

# ============================================================================
# 5. Key content in DEPLOYMENT.md and NODE docs
# ============================================================================
section "Key content in docs"

for needle in "git pull" "./deploy.sh" "docker-compose.release.yaml"; do
    if grep -q -- "$needle" docs/DEPLOYMENT.md 2>/dev/null; then
        ok "docs/DEPLOYMENT.md contains '$needle'"
    else
        fail "docs/DEPLOYMENT.md missing required '$needle'"
    fi
done
# Backup: 'backup', 'back up' (en) or '备份' (zh). The doc is in English; the
# upgrade flow should tell operators to back up DB + .env first.
if grep -qiE "back ?up|备份" docs/DEPLOYMENT.md 2>/dev/null; then
    ok "docs/DEPLOYMENT.md mentions backup"
else
    fail "docs/DEPLOYMENT.md missing 'backup' (the upgrade flow should mention backing up DB + .env)"
fi

# NODE docs must mention binaries, install paths, status, version command, install script
for doc in docs/NODE.md docs/NODE.zh-CN.md; do
    [ -f "$doc" ] || { fail "$doc missing"; continue; }
    for needle in "relay-node-linux-amd64" "relay-node-linux-arm64" "/opt/relay-node" "systemctl status relay-node" "/opt/relay-node/relay-node --version" "relay-node-install.sh"; do
        if grep -q -- "$needle" "$doc" 2>/dev/null; then
            ok "$doc contains '$needle'"
        else
            fail "$doc missing required content: '$needle'"
        fi
    done
done

# README must link to key docs. Content-keyword checks (relay-node-install.sh,
# device groups, install/upgrade phrasing) are WARN since the slim README
# intentionally delegates detail to docs/ — only the doc LINKS are hard FAILs.
for readme in README.md README.en.md; do
    [ -f "$readme" ] || { fail "$readme missing"; continue; }
    # Hard: README must link to the deployment guide (primary navigation).
    if grep -q -- "docs/DEPLOYMENT.md" "$readme" 2>/dev/null; then
        ok "$readme links to docs/DEPLOYMENT.md"
    else
        fail "$readme: no link to docs/DEPLOYMENT.md"
    fi
    # Soft: the slim README routes everything through DEPLOYMENT.md as the single
    # entry point; a separate node-doc link is nice-to-have, not required.
    if grep -qE "docs/NODE\.md|docs/NODE\.zh-CN\.md" "$readme" 2>/dev/null; then
        ok "$readme links to a node doc"
    else
        warn "$readme has no direct node-doc link (ok — DEPLOYMENT.md covers it)"
    fi
    # Soft (WARN): the slim README may not literally mention these strings —
    # they live in docs/ now. Warn so a regression is noticed, but don't block.
    for needle in "relay-node-install.sh"; do
        if grep -q -- "$needle" "$readme" 2>/dev/null; then
            ok "$readme contains '$needle'"
        else
            warn "$readme no longer mentions '$needle' (ok if slimmed; lives in docs/)"
        fi
    done
    if grep -qE "Device Groups|设备分组" "$readme" 2>/dev/null; then
        ok "$readme mentions Device Groups / 设备分组"
    else
        warn "$readme no longer mentions Device Groups / 设备分组 (ok if slimmed)"
    fi
    if grep -qE "install and upgrade|installs and upgrades|安装和升级|兼顾安装与升级" "$readme" 2>/dev/null; then
        ok "$readme mentions install and upgrade / 安装和升级"
    else
        warn "$readme no longer mentions install/upgrade phrasing (ok if slimmed)"
    fi
done

# ============================================================================
# 6. Executable permission warnings (don't auto-chmod, just warn)
# ============================================================================
section "Executable permissions"

# Check BOTH the git-stored mode (what a fresh clone gets) AND the working-tree
# bit. The git mode is the one that matters for users: if it's 100644 in the
# repo, `git clone` produces a non-executable file and ./deploy.sh fails with
# "Permission denied" even after `chmod +x` locally (the next checkout reverts
# it). So a 100644 git mode is a hard FAIL, not a warning.
for f in install.sh deploy.sh scripts/deploy-web-mode-check.sh scripts/relay-node-install.sh; do
    if [ ! -f "$f" ]; then
        fail "$f does not exist"
        continue
    fi
    # Working-tree executable bit (catches a local chmod slip).
    if [ -x "$f" ]; then
        ok "$f is executable (working tree)"
    else
        fail "$f is NOT executable in the working tree (run: chmod +x $f)"
    fi
    # Git-stored mode — the source of truth for clones. Must be 100755.
    git_mode=$(git ls-files --stage -- "$f" 2>/dev/null | awk '{print $1}')
    if [ "$git_mode" = "100755" ]; then
        ok "$f git mode is 100755 (executable after clone)"
    elif [ -z "$git_mode" ] && git ls-files --others --exclude-standard -- "$f" | grep -q .; then
        warn "$f is untracked; add it with executable mode before commit (git add --chmod=+x $f)"
    else
        fail "$f git mode is '$git_mode' (expected 100755). Fix with: git update-index --chmod=+x $f"
    fi
    # LF line endings. CRLF makes bash on Linux misparse (and shellcheck flags
    # SC1017). .gitattributes enforces this, but check anyway in case someone
    # committed with autocrlf and no attributes.
    if grep -q $'\r' "$f" 2>/dev/null; then
        fail "$f has CRLF line endings (needs LF). Fix: sed -i 's/\r$//' $f"
    else
        ok "$f uses LF line endings"
    fi
done

# ============================================================================
# Summary
# ============================================================================
echo ""
echo "================================="
echo "  Summary: $OK OK, $WARN WARN, $FAIL FAIL"
echo "================================="

if [ "$FAIL" -gt 0 ]; then
    echo ""
    echo "FAIL items must be fixed before tagging the release."
    echo "See docs/VERSIONS.md for the version-sync checklist."
    exit 1
fi

if [ "$WARN" -gt 0 ]; then
    echo ""
    echo "WARN items are non-blocking but worth reviewing."
fi

echo ""
echo "OK to tag: git tag ${TAG} && git push origin ${TAG}"
echo "(track: ${TRACK})"
exit 0
