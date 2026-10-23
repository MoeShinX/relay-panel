#!/usr/bin/env bash
#
# RelayPanel one-line installer for Linux (Debian / Ubuntu).
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/MoeShinX/relay-panel/main/install.sh | bash
#
# Where GitHub is slow or blocked (mainland China), the repository and the
# container images are fetched through download mirrors by themselves when
# GitHub / ghcr.io do not answer; what a mirror delivers is checked against the
# commit id / image digest that GitHub or a trusted relay reported.
#
# Running it again on an installed panel updates it (fast-forward only).
#

# Touch: trigger script-check CI on version-only PRs (paths filter otherwise skips it).
# What it does:
#   1. Verifies Linux + root
#   2. Installs git, curl, ca-certificates, openssl (apt)
#   3. Clones (or updates) the repo into /opt/relay-panel (quiet by default;
#      set DEBUG=1 to see raw git output)
#   4. Runs deploy.sh inside that directory
#
# NOTE: This is the one-line PANEL installer. It is distinct from
# scripts/relay-node-install.sh, which installs only the relay-node binary on
# a forwarding node. This script was accidentally emptied in commit b03ae7a
# and shipped empty through v0.1.7; restored verbatim from f184fb8 in v0.1.8.
#
set -euo pipefail

INSTALL_DIR="/opt/relay-panel"
REPO_URL="https://github.com/MoeShinX/relay-panel.git"

# DEBUG=1 shows full git output during clone/pull (default: quiet, clean status
# lines only). Most users want a clean install log without a wall of git diff
# stats, so quiet is the default.
DEBUG="${DEBUG:-0}"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

info()  { echo -e "${GREEN}[INFO]${NC}  $*"; }
warn()  { echo -e "${YELLOW}[WARN]${NC}  $*"; }
fail()  { echo -e "${RED}[FAIL]${NC}  $*"; exit 1; }

# Quiet git flags: suppress the file-change stat dump (the `Foo | 123 +++`
# block) that looks like an error to non-technical users. When DEBUG=1 we drop
# --quiet so the full output is visible for troubleshooting.
if [ "$DEBUG" = "1" ]; then
    GIT_QUIET=""
else
    GIT_QUIET="--quiet"
fi

# ---------- 1. Platform check ----------
if [ "$(uname -s)" != "Linux" ]; then
    fail "This installer only runs on Linux. Current OS: $(uname -s)"
fi

if [ "$(id -u)" -ne 0 ]; then
    warn "Not running as root. Re-run with sudo if you hit permission errors."
    warn "Install target is ${INSTALL_DIR} (needs root to write)."
fi

# ---------- 2. Install base deps ----------
need_cmd() { command -v "$1" >/dev/null 2>&1; }

# Detect package manager
PKG_MANAGER=""
if need_cmd apt-get; then
    PKG_MANAGER="apt"
elif need_cmd dnf; then
    PKG_MANAGER="dnf"
elif need_cmd yum; then
    PKG_MANAGER="yum"
else
    fail "No supported package manager found (apt/dnf/yum). Currently only Debian/Ubuntu is supported."
fi

if [ "$PKG_MANAGER" != "apt" ]; then
    warn "Detected ${PKG_MANAGER}. This installer is tested on Debian/Ubuntu."
    warn "Proceeding, but dependency names may differ."
fi

MISSING=""
for cmd in git curl ca-certificates openssl; do
    # ca-certificates is a package name, not a command - check differently
    if [ "$cmd" = "ca-certificates" ] || [ "$cmd" = "openssl" ]; then
        if [ "$cmd" = "ca-certificates" ] && [ ! -d /usr/share/ca-certificates ] && [ ! -d /etc/ssl/certs ]; then
            MISSING="$MISSING $cmd"
        elif [ "$cmd" = "openssl" ] && ! need_cmd openssl; then
            MISSING="$MISSING $cmd"
        fi
    elif ! need_cmd "$cmd"; then
        MISSING="$MISSING $cmd"
    fi
done

if [ -n "$MISSING" ]; then
    info "Installing missing dependencies:${MISSING}"
    case "$PKG_MANAGER" in
        apt)
            export DEBIAN_FRONTEND=noninteractive
            apt-get update -qq
            apt-get install -y -qq git curl ca-certificates openssl
            ;;
        dnf)
            dnf install -y git curl ca-certificates openssl
            ;;
        yum)
            yum install -y git curl ca-certificates openssl
            ;;
    esac
    info "Dependencies installed."
else
    info "All base dependencies present."
fi

# ---------- 3. Clone or update repo ----------
# Git output is kept quiet by default (see GIT_QUIET above): the file-change
# stat block that `git pull` prints looks like an error to most users, and the
# one-line status below is all they need. Set DEBUG=1 to see raw git output.
#
# GitHub is used directly first. Where it is slow or blocked, the same URL is
# tried behind download mirrors (a prefix in front of the full GitHub URL).
# Same list, order and timeouts as scripts/mirror-lib.sh, which cannot be
# sourced here because the repository does not exist yet; keep the two (and
# crates/panel/src/api/system.rs) in sync.
#
# What is fetched ends up running as root (deploy.sh), so mirrors never decide
# what the content is:
#   - TRUSTED sources are GitHub itself and a trusted relay (the first two
#     entries). Only they are asked for the commit id of main.
#   - Every route, the public ones (gh-proxy.com, ghfast.top) included, is then
#     asked for exactly that commit, and what arrives must BE that commit: git
#     ids are content hashes, so a match proves the content. No trusted answer
#     means nothing is fetched at all.
GITHUB_PREFIXES=(
    ""
    "https://down.relaypanel.dev/"
    "https://gh-proxy.com/"
    "https://ghfast.top/"
)
TRUSTED_COUNT=2
GIT_ERR="/tmp/relaypanel-git-err"
# Give up on a transfer that stays under 50 KB/s for 15 s; never ask for a
# password on a mirror that answers 401.
export GIT_HTTP_LOW_SPEED_LIMIT=51200 GIT_HTTP_LOW_SPEED_TIME=15 GIT_TERMINAL_PROMPT=0

# Can this route serve the repository? 5 s to connect, 10 s in total.
route_reachable() { # FULL_REPO_URL
    curl -fsS -o /dev/null --connect-timeout 5 --max-time 10 \
        "${1}/info/refs?service=git-upload-pack" >/dev/null 2>&1
}

# A name for messages: the relay is shown as "mirror", never by its address.
route_label() { # INDEX PREFIX
    case "$1" in
        0) echo "GitHub" ;;
        1) echo "mirror" ;;
        *)
            local host="${2#https://}"
            echo "${host%%/*}"
            ;;
    esac
}

# git's own messages quote the URL it was given; do not show the relay's
# address in them.
show_git_err() {
    sed "s#${GITHUB_PREFIXES[1]}#(mirror) #g" "$GIT_ERR" >&2
}

# The commit id of refs/heads/main from a TRUSTED source only.
trusted_main_sha() {
    local i prefix label out sha ref
    for ((i = 0; i < TRUSTED_COUNT; i++)); do
        prefix="${GITHUB_PREFIXES[$i]}"
        label="$(route_label "$i" "$prefix")"
        if ! route_reachable "${prefix}${REPO_URL}"; then
            warn "${label} is not reachable from here, trying the next source ..." >&2
            continue
        fi
        if command -v timeout >/dev/null 2>&1; then
            out="$(timeout 10 git ls-remote "${prefix}${REPO_URL}" refs/heads/main 2>/dev/null)" || out=""
        else
            out="$(git ls-remote "${prefix}${REPO_URL}" refs/heads/main 2>/dev/null)" || out=""
        fi
        while read -r sha ref; do
            if [ "$ref" = "refs/heads/main" ] && [[ "$sha" =~ ^[0-9a-f]{40}$ ]]; then
                echo "$sha"
                return 0
            fi
        done <<<"$out"
        warn "${label} did not give the commit id of main, trying the next source ..." >&2
    done
    return 1
}

# github_try CALLBACK — run CALLBACK FULL_REPO_URL on each route in order until
# it succeeds (return 0). The callback leaves git's error in $GIT_ERR.
github_try() {
    local cb="$1" prefix label i
    : > "$GIT_ERR"
    for i in "${!GITHUB_PREFIXES[@]}"; do
        prefix="${GITHUB_PREFIXES[$i]}"
        label="$(route_label "$i" "$prefix")"
        if ! route_reachable "${prefix}${REPO_URL}"; then
            warn "${label} is not reachable from here, trying the next route ..."
            continue
        fi
        [ -z "$prefix" ] || info "Using ${label} ..."
        if "$cb" "${prefix}${REPO_URL}"; then
            return 0
        fi
        warn "${label} failed, trying the next route ..."
    done
    return 1
}

# Fetch WANT_SHA (an exact commit) into the current repository, and require
# that FETCH_HEAD is that commit. FETCH_DEPTH is "" or "--depth 1".
WANT_SHA=""
FETCH_DEPTH=""
do_fetch() { # FULL_REPO_URL
    # shellcheck disable=SC2086 # FETCH_DEPTH is deliberately word-split
    git fetch $GIT_QUIET $FETCH_DEPTH "$1" "$WANT_SHA" 2>"$GIT_ERR" || return 1
    if [ "$(git rev-parse --verify --quiet 'FETCH_HEAD^{commit}' 2>/dev/null)" != "$WANT_SHA" ]; then
        echo "the route delivered a different commit than GitHub reported for main; rejected" > "$GIT_ERR"
        return 1
    fi
}

no_trusted_sha() {
    fail "Could not get the current commit of main from GitHub or the trusted mirror. Mirrors are not used without a commit id to check their data against; nothing was changed. Check your network and re-run."
}

if [ -d "$INSTALL_DIR" ]; then
    if [ -d "$INSTALL_DIR/.git" ]; then
        info "Repository exists at ${INSTALL_DIR} - checking for updates ..."
        cd "$INSTALL_DIR"
        before=$(git rev-parse --short HEAD 2>/dev/null || echo '?')
        WANT_SHA="$(trusted_main_sha)" || no_trusted_sha
        if ! github_try do_fetch; then
            echo -e "${RED}[FAIL]${NC} Could not fetch main. git output:" >&2
            show_git_err
            rm -f "$GIT_ERR"
            fail "Could not reach GitHub, directly or through any download mirror. Check your network; the repository was not changed."
        fi
        git update-ref refs/remotes/origin/main "$WANT_SHA" || true
        # --ff-only: never create a merge commit during an automated update.
        # Capture stderr so a failure (e.g. diverged history) prints the real
        # git message instead of a generic "update failed".
        if ! git merge --ff-only $GIT_QUIET "$WANT_SHA" 2>"$GIT_ERR"; then
            echo -e "${RED}[FAIL]${NC} Repository update failed. git output:" >&2
            show_git_err
            rm -f "$GIT_ERR"
            fail "git update failed. If your local branch diverged, reset with: git -C $INSTALL_DIR reset --hard origin/main"
        fi
        rm -f "$GIT_ERR"
        after=$(git rev-parse --short HEAD 2>/dev/null || echo '?')
        if [ "$before" = "$after" ]; then
            info "Repository already up to date. ($after)"
        else
            info "Repository updated: $before -> $after"
        fi
    else
        fail "${INSTALL_DIR} exists but is not a git repository. \
Back it up or remove it, then re-run this script. Refusing to overwrite."
    fi
else
    info "Cloning ${REPO_URL} into ${INSTALL_DIR} ..."
    WANT_SHA="$(trusted_main_sha)" || no_trusted_sha
    # Not `git clone`: the commit is fetched by id and verified, then checked
    # out. origin is GitHub whichever route delivered it.
    git -c init.defaultBranch=main init -q "$INSTALL_DIR"
    cd "$INSTALL_DIR"
    git remote add origin "$REPO_URL"
    FETCH_DEPTH="--depth 1"
    if ! github_try do_fetch; then
        echo -e "${RED}[FAIL]${NC} Clone failed. git output:" >&2
        show_git_err
        rm -f "$GIT_ERR"
        # INSTALL_DIR did not exist before this script created it.
        cd /
        rm -rf -- "$INSTALL_DIR"
        fail "git clone failed. Could not reach GitHub, directly or through any download mirror. Check your network."
    fi
    rm -f "$GIT_ERR"
    git update-ref refs/remotes/origin/main "$WANT_SHA"
    git checkout -q -B main "$WANT_SHA"
    git branch -q --set-upstream-to=origin/main main >/dev/null 2>&1 || true
    info "Cloned at $(git rev-parse --short HEAD 2>/dev/null || echo '?')"
fi

# ---------- 4. Hand off to deploy.sh ----------
if [ ! -f deploy.sh ]; then
    fail "deploy.sh not found in ${INSTALL_DIR}. Repository may be incomplete."
fi

chmod +x deploy.sh
info "Starting deploy.sh ..."
exec ./deploy.sh "$INSTALL_DIR"
# maintenance comment to retrigger script-check
