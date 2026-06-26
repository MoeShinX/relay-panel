#!/usr/bin/env bash
#
# Pre-release consistency check.
#
# Run BEFORE tagging a new release. Verifies that every place a version number
# lives in the repo agrees with the version you pass on the command line.
# Does NOT modify any file. Exits 1 on FAIL (so it can plug into CI later).
#
# Usage:
#   bash scripts/release-check.sh 0.2.1
#   bash scripts/release-check.sh v0.2.1
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
if [ $# -lt 1 ]; then
    echo "Usage: bash scripts/release-check.sh <version>"
    echo "  e.g. bash scripts/release-check.sh 0.2.1"
    echo "       bash scripts/release-check.sh v0.2.1"
    exit 1
fi

RAW="$1"
# Normalize: strip leading 'v' or 'V' if present.
VERSION="${RAW#[vV]}"
# A version must be 3 dot-separated numbers, optionally followed by a SemVer
# pre-release suffix (-alpha, -beta.1, -rc.2, …). We accept both stable
# (0.3.0) and pre-release (0.3.0-alpha) forms so the check works for alpha/beta
# cuts too. Build metadata (+build) is rejected to keep it simple.
if ! [[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?$ ]]; then
    echo "ERROR: '$RAW' is not a valid semver-like version (expected x.y.z, vx.y.z, x.y.z-suffix, or vx.y.z-suffix)"
    exit 1
fi
TAG="v${VERSION}"

# Find the repo root (this script lives in <root>/scripts/).
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$ROOT"

echo "Release pre-flight check for version: $VERSION (tag: $TAG)"
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
# 2. Version string consistency (the 6 places from docs/VERSIONS.md)
# ============================================================================
section "Version consistency"

# 2.1 relay-node: crates/node/Cargo.toml
NODE_VER=$(toml_value "crates/node/Cargo.toml" "version")
if [ -z "$NODE_VER" ]; then
    fail "crates/node/Cargo.toml: no version found"
elif [ "$NODE_VER" = "$VERSION" ]; then
    ok "crates/node/Cargo.toml version = $NODE_VER"
else
    fail "crates/node/Cargo.toml version = $NODE_VER (expected $VERSION)"
fi

# 2.2 panel: crates/panel/src/config.rs (COMPILED_APP_VERSION)
PANEL_VER=$(rust_const_value "crates/panel/src/config.rs" "COMPILED_APP_VERSION")
if [ -z "$PANEL_VER" ]; then
    fail "crates/panel/src/config.rs: COMPILED_APP_VERSION not found"
elif [ "$PANEL_VER" = "$VERSION" ]; then
    ok "crates/panel/src/config.rs COMPILED_APP_VERSION = $PANEL_VER"
else
    fail "crates/panel/src/config.rs COMPILED_APP_VERSION = $PANEL_VER (expected $VERSION)"
fi

# 2.3 relay-node install script: scripts/relay-node-install.sh (SCRIPT_VERSION="X")
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

# 2.4 docker-compose.release.yaml: both image tags
if grep -q "ghcr.io/moeshinx/relay-panel-panel:${VERSION}\b" docker-compose.release.yaml 2>/dev/null; then
    ok "docker-compose.release.yaml panel image tag = ${VERSION}"
else
    fail "docker-compose.release.yaml: panel image tag ${VERSION} not found"
fi
if grep -q "ghcr.io/moeshinx/relay-panel-node:${VERSION}\b" docker-compose.release.yaml 2>/dev/null; then
    ok "docker-compose.release.yaml node image tag = ${VERSION}"
else
    fail "docker-compose.release.yaml: node image tag ${VERSION} not found"
fi

# 2.5 README release badge. Both READMEs use a dynamic shields.io
# `github/v/release` badge that always reflects the latest GitHub release,
# so there is no static version string to bump. Just confirm the badge is present.
for rf in README.md README.en.md; do
    if grep -q "github/v/release" "$rf" 2>/dev/null; then
        ok "$rf has dynamic release badge"
    else
        fail "$rf: dynamic release badge (github/v/release) not found"
    fi
done

# 2.6 CHANGELOG
if grep -qE "^\#\#\s*\[${VERSION}\](\s*-|$)" CHANGELOG.md 2>/dev/null; then
    ok "CHANGELOG.md has section [${VERSION}]"
else
    fail "CHANGELOG.md: no '## [${VERSION}]' section"
fi

# ============================================================================
# 3. Cargo supplementary checks
# ============================================================================
section "Cargo supplementary"

# 3.1 root Cargo.toml workspace.package.version (optional)
ROOT_WS_VER=$(toml_value "Cargo.toml" "version")
if [ -n "$ROOT_WS_VER" ]; then
    if [ "$ROOT_WS_VER" = "$VERSION" ]; then
        ok "workspace.package.version = $ROOT_WS_VER"
    else
        fail "workspace.package.version = $ROOT_WS_VER (expected $VERSION)"
    fi
else
    ok "no workspace.package.version in root Cargo.toml (skipped)"
fi

# 3.2 crates/panel/Cargo.toml - HARD FAIL on mismatch.
# v0.3.5: the panel crate version IS part of the release-sync set now. It was
# missed in v0.3.4 (stayed 0.3.3 while everything else moved to 0.3.4), so it
# is promoted from WARN to FAIL to make that class of slip impossible.
PANEL_TOML_VER=$(toml_value "crates/panel/Cargo.toml" "version")
if [ -n "$PANEL_TOML_VER" ]; then
    if [ "$PANEL_TOML_VER" = "$VERSION" ]; then
        ok "crates/panel/Cargo.toml version = $PANEL_TOML_VER"
    else
        fail "crates/panel/Cargo.toml version = $PANEL_TOML_VER (expected $VERSION)"
    fi
else
    fail "crates/panel/Cargo.toml: no version field (expected $VERSION)"
fi

# 3.3 crates/shared/Cargo.toml - WARN if not in sync
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

# 3.4 Cargo.lock: project package versions (avoid 3rd-party collisions)
# For each known package name, walk to its version line in the same [[package]] block.
# relay-node AND relay-panel are release-sync'd -> hard FAIL on mismatch (panel
# was promoted in v0.3.5). relay-shared is NOT bumped per release, so a mismatch
# there is only a WARN.
PROJECT_PKGS=("relay-node" "relay-panel" "relay-shared")
for pkg in "${PROJECT_PKGS[@]}"; do
    # Use awk for the small state machine (we keep this awk because it has no
    # quoted regex body — just a single line, no `/.../` style patterns).
    LOCK_VER=$(awk -v pkg="$pkg" '
        /^name = / {
            if ($0 == "name = \"" pkg "\"") { found=1; next }
        }
        found && /^version = / {
            # Extract the quoted version value
            s = $0
            i = index(s, "\"")
            if (i > 0) {
                j = index(substr(s, i + 1), "\"")
                if (j > 0) print substr(s, i + 1, j - 1)
            }
            found = 0
        }
    ' Cargo.lock 2>/dev/null || true)
    if [ -z "$LOCK_VER" ]; then
        warn "Cargo.lock: $pkg not found (workspace may not include this crate?)"
    elif [ "$LOCK_VER" = "$VERSION" ]; then
        ok "Cargo.lock $pkg = $LOCK_VER"
    elif [ "$pkg" = "relay-node" ] || [ "$pkg" = "relay-panel" ]; then
        fail "Cargo.lock $pkg = $LOCK_VER (expected $VERSION)"
    else
        warn "Cargo.lock $pkg = $LOCK_VER ($pkg crate version is not used as release source yet.)"
    fi
done

# 3.5 CHANGELOG section must be non-empty (not just present). A blank section
# would publish an empty GitHub Release body (the v0.3.4 body=null bug). The
# shared extractor exits non-zero on a missing / whitespace-only section.
if bash scripts/extract-changelog.sh "$VERSION" >/dev/null 2>&1; then
    ok "CHANGELOG.md [${VERSION}] section is non-empty (release body will not be blank)"
else
    fail "CHANGELOG.md [${VERSION}] section is missing or empty (would publish an empty release body)"
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
exit 0
