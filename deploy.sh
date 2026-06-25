#!/usr/bin/env bash
#
# RelayPanel one-click deployment script for Linux (Debian/Ubuntu).
#
# Usage:
#   ./deploy.sh              # pull pre-built images from GHCR (fast, default)
#   ./deploy.sh /path/to/dir # same, but use the given repo directory
#   RELAYPANEL_BUILD_LOCAL=1 ./deploy.sh
#                            # build from source instead of pulling images
#
# What it does:
#   1. Checks Docker + docker compose are installed (installs via get.docker.com if not)
#   2. Generates JWT_SECRET + PANEL_KEY into .env (skips if .env already exists)
#   3. Pulls pre-built images from GHCR (or builds from source if BUILD_LOCAL=1)
#   4. docker compose up -d
#   5. Waits for panel HTTP, then checks GET /api/v1/health (a real JSON health
#      probe — requires HTTP 200 + application/json + status:"ok" + a version).
#      Does NOT probe the default admin password (that was a needless login
#      attempt that could trip rate-limiting and never proved anything the
#      health endpoint doesn't already prove).
#
# NOTE: This script was accidentally emptied in commit b03ae7a (ASCII-only
# rewrite) and shipped empty through v0.1.7, which caused `./deploy.sh` to
# exit 0 without doing anything. Restored verbatim from cf1c193 in v0.1.8.
#
set -euo pipefail

REPO_DIR="${1:-$(cd "$(dirname "$0")" && pwd)}"
cd "$REPO_DIR"

GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m'

info()  { echo -e "${GREEN}[INFO]${NC}  $*"; }
warn()  { echo -e "${YELLOW}[WARN]${NC}  $*"; }
fail()  { echo -e "${RED}[FAIL]${NC}  $*"; exit 1; }

# Read a single KEY's value from .env WITHOUT sourcing it. Sourcing (`. ./.env`)
# executes the file as shell, so a value containing &, $(...), backticks, spaces
# or quotes — all legal in a PostgreSQL DSN like
# `postgres://u:p@h/db?sslmode=require&application_name=relaypanel` — would be
# interpreted (or even executed) instead of read literally. We grep the last
# matching KEY=VALUE line and strip the key prefix plus one optional layer of
# surrounding quotes. Prints nothing if the key is absent.
env_get() {
    key="$1"
    [ -f .env ] || return 0
    line=$(grep -E "^${key}=" .env 2>/dev/null | tail -n1) || return 0
    [ -n "$line" ] || return 0
    val="${line#"${key}="}"
    case "$val" in
        \"*\") val="${val#\"}"; val="${val%\"}" ;;
        \'*\') val="${val#\'}"; val="${val%\'}" ;;
    esac
    printf '%s' "$val"
}

# Redact the password in a postgres:// URL for safe logging:
#   postgres://user:secret@host:5432/db?x=1 -> postgres://user:***@host:5432/db?x=1
# Never print a raw DATABASE_URL — it carries credentials that would otherwise
# land in terminal scrollback and CI logs.
redact_url() {
    printf '%s' "$1" | sed -E 's#(://[^:/@]+:)[^@]*@#\1***@#'
}

# ---------- 1. Docker ----------
if ! command -v docker >/dev/null 2>&1; then
    info "Docker not found - installing via get.docker.com ..."
    curl -fsSL https://get.docker.com | sh
    systemctl enable --now docker
    usermod -aG docker "$USER" 2>/dev/null || true
    warn "Docker installed. You may need to log out/in for group changes."
fi

if ! docker compose version >/dev/null 2>&1; then
    fail "docker compose plugin not found. Install: apt-get install docker-compose-plugin"
fi
info "Docker OK: $(docker --version)"

# ---------- 2. .env ----------
# FRESH_INSTALL distinguishes a brand-new install (no .env yet) from an upgrade
# (.env already present). Only fresh installs get the interactive database
# backend menu; upgrades keep whatever backend they already use, untouched.
if [ ! -f .env ]; then
    FRESH_INSTALL=1
    info "Generating secrets into .env ..."
    JWT_SECRET=$(openssl rand -hex 32)
    PANEL_KEY=$(openssl rand -hex 16)
    cat > .env <<EOF
JWT_SECRET=${JWT_SECRET}
PANEL_KEY=${PANEL_KEY}
NODE_TOKEN=change-me-after-creating-a-group
EOF
    chmod 600 .env
    info ".env created (mode 600)"
else
    FRESH_INSTALL=0
    warn ".env already exists - skipping secret generation"
fi

# ---------- 2b. Database backend resolution ----------
# The interactive backend MENU is shown ONLY on a brand-new install. Upgrades
# keep whatever backend they already use, untouched, and never see a switch
# option (switching SQLite<->PostgreSQL requires a manual stop + data migration
# by the operator, by design).
#
# Resolution precedence:
#   (A) DATABASE_URL from the environment (CI / pre-configured)  — used as-is
#   (B) Upgrade (.env already existed before this run):
#         1. DATABASE_URL stored in .env
#         2. Legacy DATABASE_PATH (pre-v0.4.3) — wrapped into a sqlite: URL at
#            the SAME location (the data file is never moved)
#         3. Neither present — pre-v0.4.3 default SQLite path
#       In all upgrade cases: no prompt, DATABASE_URL/RELAYPANEL_DB_MODE are
#       left unchanged, and one concise "(unchanged)" line is printed.
#   (C) Fresh install: interactive menu via /dev/tty (works under curl|bash),
#       or SQLite default when there is no terminal.
#
# Security boundaries:
#   - Upgrades NEVER offer to switch backends.
#   - PostgreSQL connectivity failure is fatal (handled in 2c) — never fall
#     back to SQLite.
#   - .env is never sourced (special chars in a DSN would break or execute).
#   - PostgreSQL URLs are redacted before logging (see redact_url).

# Human-readable backend label for the concise upgrade message.
db_backend_label() {
    case "${RELAYPANEL_DB_MODE:-}" in
        embedded-postgres) printf 'PostgreSQL embedded' ;;
        external-postgres) printf 'PostgreSQL external' ;;
        *)
            case "${1:-}" in
                postgres://*|postgresql://*) printf 'PostgreSQL' ;;
                *) printf 'SQLite' ;;
            esac
            ;;
    esac
}

if [ -n "${DATABASE_URL:-}" ]; then
    # (A) Provided by the environment — authoritative, used as-is.
    info "DATABASE_URL provided via environment — using it as-is"
    # Fill RELAYPANEL_DB_MODE from .env only if the caller didn't set it; never
    # touch the caller's DATABASE_URL.
    if [ -z "${RELAYPANEL_DB_MODE:-}" ]; then
        RELAYPANEL_DB_MODE="$(env_get RELAYPANEL_DB_MODE)"
    fi
    if ! grep -q '^DATABASE_URL=' .env 2>/dev/null; then
        cat >> .env <<EOF
DATABASE_URL=${DATABASE_URL}
EOF
        chmod 600 .env
    fi
elif [ "$FRESH_INSTALL" = "0" ]; then
    # (B) Upgrade — detect and keep the existing backend. Never prompt.
    if grep -q '^DATABASE_URL=' .env 2>/dev/null; then
        DATABASE_URL="$(env_get DATABASE_URL)"
        RELAYPANEL_DB_MODE="${RELAYPANEL_DB_MODE:-$(env_get RELAYPANEL_DB_MODE)}"
    elif grep -q '^DATABASE_PATH=' .env 2>/dev/null; then
        # Legacy pre-v0.4.3 config: wrap DATABASE_PATH into a sqlite: URL at the
        # SAME location. We must NOT relocate the existing data file.
        _LEGACY_PATH="$(env_get DATABASE_PATH)"
        DATABASE_URL="sqlite:${_LEGACY_PATH}?mode=rwc"
        cat >> .env <<EOF
DATABASE_URL=${DATABASE_URL}
EOF
        chmod 600 .env
    else
        # No backend recorded at all — the pre-v0.4.3 default was SQLite here.
        DATABASE_URL="sqlite:/app/data/data.db?mode=rwc"
        cat >> .env <<EOF
DATABASE_URL=${DATABASE_URL}
EOF
        chmod 600 .env
    fi
    info "Existing installation detected."
    info "Database backend: $(db_backend_label "$DATABASE_URL") (unchanged)"
else
    # (C) Fresh install — interactive menu if we have a terminal.
    if [ -t 0 ] || [ -t 1 ]; then
        # Attach a terminal so prompts work even under curl|bash.
        exec < /dev/tty || true
    fi

    if [ -t 0 ]; then
        echo ""
        info "Database backend selection"
        echo "  1) SQLite (default, zero-config)"
        echo "  2) PostgreSQL (embedded Docker container)"
        echo ""
        printf "Choose [1-2] (default 1): "
        read -r DB_CHOICE
        DB_CHOICE="${DB_CHOICE:-1}"

        case "$DB_CHOICE" in
            1)
                info "Selected: SQLite"
                DATABASE_URL="sqlite:/app/data/data.db?mode=rwc"
                ;;
            2)
                info "Selected: PostgreSQL (embedded Docker)"
                printf "  Database name [relaypanel]: "
                read -r PG_DB
                PG_DB="${PG_DB:-relaypanel}"
                printf "  Username [relaypanel]: "
                read -r PG_USER
                PG_USER="${PG_USER:-relaypanel}"
                # Auto-generate a hex password to avoid URL-escaping issues.
                PG_PASS=$(openssl rand -hex 16)
                echo "  Password (auto-generated): ${PG_PASS}"
                DATABASE_URL="postgres://${PG_USER}:${PG_PASS}@postgres:5432/${PG_DB}"
                cat >> .env <<EOF
POSTGRES_DB=${PG_DB}
POSTGRES_USER=${PG_USER}
POSTGRES_PASSWORD=${PG_PASS}
RELAYPANEL_DB_MODE=embedded-postgres
EOF
                ;;
            *)
                fail "Invalid choice: $DB_CHOICE"
                ;;
        esac

        # Write DATABASE_URL into .env so future runs don't re-prompt.
        cat >> .env <<EOF
DATABASE_URL=${DATABASE_URL}
EOF
        chmod 600 .env
        info "DATABASE_URL written to .env"
    else
        # No TTY — default to SQLite (e.g. curl|bash one-line install).
        info "No terminal detected — defaulting to SQLite"
        DATABASE_URL="sqlite:/app/data/data.db?mode=rwc"
        cat >> .env <<EOF
DATABASE_URL=${DATABASE_URL}
EOF
        chmod 600 .env
    fi
fi

# Export for compose.
export DATABASE_URL

# ---------- 2c. Database connectivity pre-flight (v0.4.3) ----------
# Before starting the panel, verify the database is reachable.
# Failure here is fatal — we never fall back to SQLite.
case "${RELAYPANEL_DB_MODE:-}" in
    embedded-postgres)
        # Start postgres container first, wait for healthy, then start panel.
        info "Starting embedded PostgreSQL container ..."
        # Determine which compose file we'll use (same logic as step 3).
        if [ "${RELAYPANEL_BUILD_LOCAL:-0}" = "1" ]; then
            _PRE_COMPOSE="docker-compose.yaml"
        elif [ -f "docker-compose.release.yaml" ]; then
            _PRE_COMPOSE="docker-compose.release.yaml"
        else
            _PRE_COMPOSE="docker-compose.yaml"
        fi
        docker compose -f "$_PRE_COMPOSE" --profile postgres up -d postgres

        info "Waiting for PostgreSQL to become healthy ..."
        for i in $(seq 1 30); do
            HEALTH=$(docker inspect --format='{{.State.Health.Status}}' \
                "$(docker compose -f "$_PRE_COMPOSE" ps -q postgres 2>/dev/null)" 2>/dev/null || echo "unknown")
            if [ "$HEALTH" = "healthy" ]; then
                info "PostgreSQL is healthy"
                break
            fi
            sleep 2
            if [ "$i" -eq 30 ]; then
                fail "PostgreSQL did not become healthy in 60s. Check: docker compose -f $_PRE_COMPOSE logs postgres"
            fi
        done
        ;;
    external-postgres)
        # Verify connectivity with a real psql query (not just pg_isready).
        # We use a temporary postgres container on the same compose network
        # so the user doesn't need psql installed on the host.
        info "Verifying external PostgreSQL connectivity ..."
        if ! docker run --rm --network host \
            postgres:16-alpine \
            psql "${DATABASE_URL}" -v ON_ERROR_STOP=1 -c "SELECT 1" >/dev/null 2>&1; then
            fail "Cannot connect to external PostgreSQL at $(redact_url "${DATABASE_URL}"). Check host, port, user, password, and database name."
        fi
        info "External PostgreSQL is reachable"
        ;;
    *)
        # SQLite — nothing to pre-flight.
        ;;
esac

# ---------- 2d. Port pre-flight helper ----------
# Check whether a host port is already in use. Uses the first available tool
# (ss, lsof, netstat). Returns 0 if the port is free, 1 if in use, 2 if no
# tool is available (skip check with a warning).
port_in_use() {
    local port="$1"
    if command -v ss >/dev/null 2>&1; then
        ss -tlnp 2>/dev/null | grep -qE "LISTEN.*:${port}\b" && return 0 || return 1
    elif command -v lsof >/dev/null 2>&1; then
        lsof -iTCP:"${port}" -sTCP:LISTEN -nP 2>/dev/null | grep -q . && return 0 || return 1
    elif command -v netstat >/dev/null 2>&1; then
        netstat -tlnp 2>/dev/null | grep -qE ":${port}\b" && return 0 || return 1
    else
        return 2
    fi
}

# ---------- 2e. Web mode resolution ----------
# Like the database backend, web mode is resolved once and persisted in .env.
# Upgrades keep whatever mode they already use; fresh installs get an
# interactive menu (or direct default when there is no terminal).
#
# Modes:
#   direct         — panel publishes 0.0.0.0:18888 (public, default)
#   reverse-proxy  — panel binds 127.0.0.1:18888 by default (same-host proxy)
#   caddy          — Compose Caddy handles TLS on 80/443, panel localhost only
#
# Resolution precedence:
#   (A) RELAYPANEL_WEB_MODE from the environment (CI / pre-configured)
#   (B) Upgrade (.env already existed): read RELAYPANEL_WEB_MODE from .env,
#       default to "direct" if not present (pre-v0.4.5 compat).
#   (C) Fresh install: interactive menu, or "direct" default without TTY.

web_mode_label() {
    case "${1:-}" in
        reverse-proxy) printf 'Reverse proxy (external)' ;;
        caddy)         printf 'Caddy (TLS + domain)' ;;
        *)             printf 'Direct (public port)' ;;
    esac
}

# Validate a domain name: reject empty, localhost, bare IPs, schemes, paths,
# spaces, and obviously invalid strings. This is a practical ACME sanity check,
# not a full RFC parser.
is_plausible_domain() {
    local d="$1"
    [ -n "$d" ] || return 1
    case "$d" in
        *://*|*/*|*\ *|localhost) return 1 ;;
    esac
    if echo "$d" | grep -qE '^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$'; then
        return 1
    fi
    if echo "$d" | grep -qE '^[0-9a-fA-F:]+:[0-9a-fA-F:]+$'; then
        return 1
    fi
    echo "$d" | grep -qE '^[A-Za-z0-9]([A-Za-z0-9-]{0,61}[A-Za-z0-9])?(\.[A-Za-z0-9]([A-Za-z0-9-]{0,61}[A-Za-z0-9])?)+$'
}

# Persist a KEY=VALUE into .env, replacing an existing line if present and
# appending otherwise. This avoids duplicate keys across repeated deploy runs.
env_set() {
    local key="$1" value="$2" found=0
    if [ -f .env ]; then
        while IFS= read -r line || [ -n "$line" ]; do
            case "$line" in
                "${key}="*)
                    printf '%s=%s\n' "$key" "$value"
                    found=1
                    ;;
                *)
                    printf '%s\n' "$line"
                    ;;
            esac
        done < .env > .env.tmp
        if [ "$found" -eq 0 ]; then
            printf '%s=%s\n' "$key" "$value" >> .env.tmp
        fi
        mv .env.tmp .env
    else
        printf '%s=%s\n' "$key" "$value" > .env
    fi
    chmod 600 .env
}

# Always load persisted runtime values before deciding. Environment variables
# still take precedence, but upgrades must not lose .env values such as
# PUBLIC_PANEL_URL.
_web_mode_env="${RELAYPANEL_WEB_MODE:-}"
_saved_web_mode="$(env_get RELAYPANEL_WEB_MODE)"
_saved_domain="$(env_get RELAYPANEL_DOMAIN)"
_saved_public_url="$(env_get PUBLIC_PANEL_URL)"
_saved_reverse_proxy_external="$(env_get REVERSE_PROXY_EXTERNAL)"
_saved_acme_email="$(env_get ACME_EMAIL)"

if [ -n "$_web_mode_env" ]; then
    case "$_web_mode_env" in
        direct|reverse-proxy|caddy) ;;
        *) fail "RELAYPANEL_WEB_MODE must be one of: direct, reverse-proxy, caddy (got '${_web_mode_env}')" ;;
    esac
    RELAYPANEL_WEB_MODE="$_web_mode_env"
    WEB_MODE_FROM_ENV=1
    info "RELAYPANEL_WEB_MODE=${RELAYPANEL_WEB_MODE} (from environment)"
elif [ "$FRESH_INSTALL" = "0" ]; then
    RELAYPANEL_WEB_MODE="${_saved_web_mode:-direct}"
    info "Web mode: $(web_mode_label "$RELAYPANEL_WEB_MODE") (unchanged)"
else
    if [ -t 0 ] || [ -t 1 ]; then
        exec < /dev/tty || true
    fi

    if [ -t 0 ]; then
        echo ""
        info "Web access mode"
        echo "  1) Direct (default) — panel port 18888 open to the network"
        echo "  2) Reverse proxy — panel on localhost only, Nginx/Caddy in front"
        echo "  3) Caddy — Compose-managed Caddy with auto-TLS + custom domain"
        echo ""
        printf "Choose [1-3] (default 1): "
        read -r WEB_CHOICE
        WEB_CHOICE="${WEB_CHOICE:-1}"

        case "$WEB_CHOICE" in
            1) RELAYPANEL_WEB_MODE="direct" ;;
            2) RELAYPANEL_WEB_MODE="reverse-proxy" ;;
            3) RELAYPANEL_WEB_MODE="caddy" ;;
            *) fail "Invalid choice: $WEB_CHOICE" ;;
        esac

        if [ "$RELAYPANEL_WEB_MODE" = "reverse-proxy" ]; then
            if [ -n "${PUBLIC_PANEL_URL:-}" ]; then
                :
            elif [ -n "$_saved_public_url" ]; then
                PUBLIC_PANEL_URL="$_saved_public_url"
            fi
            if [ -z "${PUBLIC_PANEL_URL:-}" ]; then
                warn "PUBLIC_PANEL_URL is not set. Nodes will use the browser origin to"
                warn "reach the panel. If the panel is behind a reverse proxy with a"
                warn "domain, set PUBLIC_PANEL_URL=https://your-domain in .env and re-run."
            fi
            if [ "${REVERSE_PROXY_EXTERNAL:-0}" = "1" ]; then
                warn "Separate-server reverse proxy selected: panel port will be"
                warn "published on 0.0.0.0:18888. Restrict access with a firewall"
                warn "(ufw / iptables) so only the proxy can reach the panel."
            fi
        fi

        if [ "$RELAYPANEL_WEB_MODE" = "caddy" ]; then
            printf "  Domain (e.g. panel.example.com): "
            read -r RELAYPANEL_DOMAIN
            if ! is_plausible_domain "$RELAYPANEL_DOMAIN"; then
                fail "RELAYPANEL_DOMAIN='${RELAYPANEL_DOMAIN}' must be a plain domain like panel.example.com (no scheme/path/ip/localhost)"
            fi
            env_set RELAYPANEL_DOMAIN "$RELAYPANEL_DOMAIN"

            printf "  ACME email (optional, for Let's Encrypt notices): "
            read -r ACME_EMAIL
            if [ -n "$ACME_EMAIL" ]; then
                env_set ACME_EMAIL "$ACME_EMAIL"
            fi

            if [ -n "${PUBLIC_PANEL_URL:-}" ]; then
                :
            elif [ -n "$_saved_public_url" ]; then
                PUBLIC_PANEL_URL="$_saved_public_url"
            else
                PUBLIC_PANEL_URL="https://${RELAYPANEL_DOMAIN}"
                env_set PUBLIC_PANEL_URL "$PUBLIC_PANEL_URL"
                info "PUBLIC_PANEL_URL auto-set to ${PUBLIC_PANEL_URL}"
            fi
        fi

        env_set RELAYPANEL_WEB_MODE "$RELAYPANEL_WEB_MODE"
        if [ "${REVERSE_PROXY_EXTERNAL:-0}" = "1" ]; then
            env_set REVERSE_PROXY_EXTERNAL "1"
        fi
        info "Web mode: $(web_mode_label "$RELAYPANEL_WEB_MODE")"
    else
        info "No terminal detected — defaulting to direct web mode"
        RELAYPANEL_WEB_MODE="direct"
        env_set RELAYPANEL_WEB_MODE "$RELAYPANEL_WEB_MODE"
    fi
fi

# Persist env-selected mode/domain/public URL so the next upgrade keeps the
# same behavior instead of falling back to direct mode.
if [ "${WEB_MODE_FROM_ENV:-0}" = "1" ]; then
    env_set RELAYPANEL_WEB_MODE "$RELAYPANEL_WEB_MODE"
    if [ "$RELAYPANEL_WEB_MODE" = "caddy" ] && [ -n "${RELAYPANEL_DOMAIN:-}" ]; then
        if ! is_plausible_domain "$RELAYPANEL_DOMAIN"; then
            fail "RELAYPANEL_DOMAIN='${RELAYPANEL_DOMAIN}' must be a plain domain like panel.example.com (no scheme/path/ip/localhost)"
        fi
        env_set RELAYPANEL_DOMAIN "$RELAYPANEL_DOMAIN"
    fi
    if [ -n "${PUBLIC_PANEL_URL:-}" ]; then
        env_set PUBLIC_PANEL_URL "$PUBLIC_PANEL_URL"
    fi
    if [ -n "${ACME_EMAIL:-}" ]; then
        env_set ACME_EMAIL "$ACME_EMAIL"
    fi
    if [ -n "${REVERSE_PROXY_EXTERNAL:-}" ]; then
        env_set REVERSE_PROXY_EXTERNAL "$REVERSE_PROXY_EXTERNAL"
    fi
fi

# Resolve persisted Caddy domain and PUBLIC_PANEL_URL. Never overwrite an
# existing PUBLIC_PANEL_URL during upgrades.
if [ "$RELAYPANEL_WEB_MODE" = "caddy" ]; then
    RELAYPANEL_DOMAIN="${RELAYPANEL_DOMAIN:-${_saved_domain:-}}"
    if ! is_plausible_domain "$RELAYPANEL_DOMAIN"; then
        fail "RELAYPANEL_WEB_MODE=caddy requires RELAYPANEL_DOMAIN to be a plain domain like panel.example.com"
    fi
    if ! grep -q '^RELAYPANEL_DOMAIN=' .env 2>/dev/null; then
        env_set RELAYPANEL_DOMAIN "$RELAYPANEL_DOMAIN"
    fi
    if [ -z "${PUBLIC_PANEL_URL:-}" ] && [ -n "$_saved_public_url" ]; then
        PUBLIC_PANEL_URL="$_saved_public_url"
    fi
    if [ -z "${PUBLIC_PANEL_URL:-}" ]; then
        PUBLIC_PANEL_URL="https://${RELAYPANEL_DOMAIN}"
        env_set PUBLIC_PANEL_URL "$PUBLIC_PANEL_URL"
        info "PUBLIC_PANEL_URL auto-set to ${PUBLIC_PANEL_URL}"
    fi
    ACME_EMAIL="${ACME_EMAIL:-${_saved_acme_email:-}}"
fi

# Resolve PUBLIC_PANEL_URL for all modes so compose does not receive an empty
# value that would clobber a persisted .env setting during upgrades.
PUBLIC_PANEL_URL="${PUBLIC_PANEL_URL:-${_saved_public_url:-}}"

# Resolve optional ACME email so Compose receives the persisted value on Caddy
# upgrades. Caddyfile uses this value in its global ACME account settings.
ACME_EMAIL="${ACME_EMAIL:-${_saved_acme_email:-}}"
if [ -n "$ACME_EMAIL" ]; then
    CADDY_ACME_EMAIL_DIRECTIVE="email ${ACME_EMAIL}"
else
    CADDY_ACME_EMAIL_DIRECTIVE=""
fi

# Restore persisted separate-server reverse-proxy setting unless explicitly
# overridden by the current run.
if [ -z "${REVERSE_PROXY_EXTERNAL:-}" ] && [ -n "$_saved_reverse_proxy_external" ]; then
    REVERSE_PROXY_EXTERNAL="$_saved_reverse_proxy_external"
fi

# Set the panel port binding based on web mode.
case "${RELAYPANEL_WEB_MODE}:${REVERSE_PROXY_EXTERNAL:-0}" in
    reverse-proxy:1|direct:*) RELAYPANEL_PANEL_PORT_BINDING="0.0.0.0:18888" ;;
    *)                         RELAYPANEL_PANEL_PORT_BINDING="127.0.0.1:18888" ;;
esac
export RELAYPANEL_PANEL_PORT_BINDING
export RELAYPANEL_DOMAIN
export PUBLIC_PANEL_URL
export REVERSE_PROXY_EXTERNAL
export ACME_EMAIL
export CADDY_ACME_EMAIL_DIRECTIVE

# ---------- 2f. Port conflict pre-flight ----------
# Check that required host ports are free before starting containers.
# For Caddy mode, 80/443 conflicts are fatal because ACME HTTP/TLS
# challenges and HTTPS service cannot work without those ports.
_port_check_ok=1
case "$RELAYPANEL_WEB_MODE" in
    direct|reverse-proxy)
        if port_in_use 18888; then
            _rc=0
        else
            _rc=$?
        fi
        if [ "$_rc" -eq 0 ]; then
            warn "Port 18888 is already in use. The panel may fail to bind."
            _port_check_ok=0
        elif [ "$_rc" -eq 2 ]; then
            warn "Cannot check port 18888 (no ss/lsof/netstat). Skipping port pre-flight."
        fi
        ;;
    caddy)
        for _p in 80 443; do
            if port_in_use "$_p"; then
                _rc=0
            else
                _rc=$?
            fi
            if [ "$_rc" -eq 0 ]; then
                fail "Port $_p is already in use. Caddy mode requires free 80/443 for ACME and HTTPS. Stop the conflicting service or choose reverse-proxy mode."
            elif [ "$_rc" -eq 2 ]; then
                warn "Cannot check ports (no ss/lsof/netstat). Skipping port pre-flight."
                break
            fi
        done
        if port_in_use 18888; then
            _rc=0
        else
            _rc=$?
        fi
        if [ "$_rc" -eq 0 ]; then
            warn "Port 18888 is already in use. The panel may fail to bind."
            _port_check_ok=0
        fi
        ;;
esac
if [ "$_port_check_ok" -eq 0 ]; then
    warn "One or more required ports are in use. If the deployment fails,"
    warn "stop the conflicting service and re-run deploy.sh."
fi

# ---------- 3. Decide: pre-built images vs source build + profiles ----------
RELEASE_COMPOSE="docker-compose.release.yaml"

# Profile arrays (replaces the old PROFILE_FLAGS string). Each array element
# is a separate --profile argument, avoiding whitespace/order bugs.
PROFILE_ARGS=()

# Node profile: off by default. Most deployments run relay-node on a separate
# server. Set RELAYPANEL_WITH_NODE=1 to also start the node container here.
if [ "${RELAYPANEL_WITH_NODE:-0}" = "1" ]; then
    info "RELAYPANEL_WITH_NODE=1 - will also start the node container"
    PROFILE_ARGS+=("--profile" "node")
    # Warn if NODE_TOKEN is still the placeholder — the node will 401.
    if [ "${NODE_TOKEN:-}" = "default-token" ] || [ "${NODE_TOKEN:-}" = "change-me-after-creating-a-group" ] || [ -z "${NODE_TOKEN:-}" ]; then
        warn "NODE_TOKEN is not set or is still the placeholder. The node container"
        warn "will get 401 Unauthorized. Create an inbound group in the panel UI,"
        warn "copy its token into .env as NODE_TOKEN, then re-run deploy.sh."
    fi
fi

# PostgreSQL profile: if embedded-postgres was selected, include --profile postgres.
# The postgres container is already started in step 2c; this ensures compose
# knows about it when we start the panel.
if [ "${RELAYPANEL_DB_MODE:-}" = "embedded-postgres" ]; then
    PROFILE_ARGS+=("--profile" "postgres")
fi

# Caddy profile: if web mode is caddy, include --profile caddy.
if [ "$RELAYPANEL_WEB_MODE" = "caddy" ]; then
    PROFILE_ARGS+=("--profile" "caddy")
fi

if [ "${RELAYPANEL_BUILD_LOCAL:-0}" = "1" ]; then
    # User explicitly requested source build
    info "RELAYPANEL_BUILD_LOCAL=1 - building from source ..."
    COMPOSE_FILE="docker-compose.yaml"
    COMPOSE_FLAGS="--build"
elif [ -f "$RELEASE_COMPOSE" ]; then
    # Default: pull pre-built images from GHCR (fast, no compilation).
    # The exact image version comes from docker-compose.release.yaml (e.g.
    # ghcr.io/moeshinx/relay-panel-node:0.2.1), so bumping the tag there is
    # what rolls users forward.
    info "Pulling pre-built panel image from GHCR ..."
    COMPOSE_FILE="$RELEASE_COMPOSE"
    COMPOSE_FLAGS=""
    if ! docker compose -f "$COMPOSE_FILE" "${PROFILE_ARGS[@]}" pull; then
        fail "Failed to pull images from GHCR. Check your internet connection, or build from source with: RELAYPANEL_BUILD_LOCAL=1 ./deploy.sh"
    fi
else
    # Fallback: no release compose file, build from source
    warn "docker-compose.release.yaml not found - building from source ..."
    COMPOSE_FILE="docker-compose.yaml"
    COMPOSE_FLAGS="--build"
fi

# ---------- 4. Start ----------
info "Starting services (docker compose -f $COMPOSE_FILE up -d $COMPOSE_FLAGS ${PROFILE_ARGS[*]}) ..."
docker compose -f "$COMPOSE_FILE" up -d $COMPOSE_FLAGS "${PROFILE_ARGS[@]}"

# ---------- 5. Verify ----------
# Deployment success is decided by the CONTAINER + PORT + a real health
# endpoint, NOT by logging in as admin/admin123. Existing deployments have
# almost always changed the default password, so a 401 on the default login is
# a normal condition for an upgrade — it must NOT fail the deploy.
# (Regression: v0.2.x made the admin/admin123 login a hard FAIL, so every
# upgrade on a deployment that had changed its password reported FAIL even
# though the panel was running fine on :18888.)

info "Waiting for panel to become reachable ..."
for i in $(seq 1 30); do
    if curl -sf http://127.0.0.1:18888/ >/dev/null 2>&1; then
        info "Panel HTTP reachable on :18888"
        break
    fi
    sleep 2
    if [ "$i" -eq 30 ]; then
        fail "Panel did not become reachable in 60s. Check: docker compose -f $COMPOSE_FILE logs panel"
    fi
done

# Health check: GET /api/v1/health (unauthenticated, returns JSON status+version).
#
# This is a REAL check, not a "did something answer on :18888" check. The old
# code hit /system/version (which is admin-gated) — the request fell through to
# the SPA fallback and returned index.html, so curl saw a 200 with an HTML body
# and wrongly printed "Panel health OK: <!doctype html>...".
#
# We now require ALL of:
#   - HTTP 200
#   - Content-Type: application/json  (rejects the HTML SPA fallback)
#   - body contains "status":"ok"
#   - body contains a non-empty "version"
# Anything else (HTML, 401, empty body, invalid JSON) fails the deploy.
info "Verifying panel health (GET /api/v1/health) ..."
health_code=$(curl -s -o /tmp/rp-health-body -w '%{http_code}' \
    -D /tmp/rp-health-headers \
    http://127.0.0.1:18888/api/v1/health 2>/dev/null || echo '000')
health_body=$(cat /tmp/rp-health-body 2>/dev/null || true)
health_ct=$(grep -i '^content-type:' /tmp/rp-health-headers 2>/dev/null || true)
rm -f /tmp/rp-health-body /tmp/rp-health-headers

health_ok=1
[ "$health_code" = "200" ] || health_ok=0
echo "$health_ct" | grep -qi 'application/json' || health_ok=0
echo "$health_body" | grep -q '"status"[[:space:]]*:[[:space:]]*"ok"' || health_ok=0
echo "$health_body" | grep -q '"version"[[:space:]]*:[[:space:]]*"[^"]' || health_ok=0

if [ "$health_ok" = "1" ]; then
    # Extract the version for a clean one-line status (no HTML noise).
    health_ver=$(echo "$health_body" | grep -o '"version"[[:space:]]*:[[:space:]]*"[^"]*"' | head -1 | sed 's/.*:"\([^"]*\)".*/\1/')
    info "Panel health OK (version $health_ver)"
else
    fail "Panel health check failed: expected JSON {\"status\":\"ok\",\"version\":\"...\"} from /api/v1/health, got HTTP $health_code ($health_ct): ${health_body:0:120}. Check: docker compose -f $COMPOSE_FILE logs panel"
fi

if [ "$RELAYPANEL_WEB_MODE" = "caddy" ]; then
    info "Verifying Caddy container state ..."
    caddy_id=$(docker compose -f "$COMPOSE_FILE" "${PROFILE_ARGS[@]}" ps -q caddy 2>/dev/null || true)
    if [ -z "$caddy_id" ]; then
        fail "Caddy container is not running. Check: docker compose -f $COMPOSE_FILE ${PROFILE_ARGS[*]} logs caddy"
    fi
    caddy_state=$(docker inspect --format='{{.State.Status}}' "$caddy_id" 2>/dev/null || echo unknown)
    if [ "$caddy_state" != "running" ]; then
        fail "Caddy container state is $caddy_state, expected running. Check: docker compose -f $COMPOSE_FILE ${PROFILE_ARGS[*]} logs caddy"
    fi
    info "Caddy container is running"

    info "Verifying Caddy HTTPS endpoint (https://${RELAYPANEL_DOMAIN}/) ..."
    caddy_https_ok=0
    for i in $(seq 1 30); do
        if curl -fsS "https://${RELAYPANEL_DOMAIN}/" >/dev/null 2>&1; then
            caddy_https_ok=1
            break
        fi
        sleep 2
    done
    if [ "$caddy_https_ok" != "1" ]; then
        fail "Caddy HTTPS check failed for https://${RELAYPANEL_DOMAIN}/ after 60s. Ensure DNS points to this server, ports 80/443 are reachable, and check: docker compose -f $COMPOSE_FILE ${PROFILE_ARGS[*]} logs caddy"
    fi
    info "Caddy HTTPS endpoint OK"
fi

echo ""
info "=========================================="
info " RelayPanel is running!"
info "=========================================="
# Build the URL line based on web mode.
case "$RELAYPANEL_WEB_MODE" in
    caddy)
        echo "  URL:     https://${RELAYPANEL_DOMAIN}"
        ;;
    reverse-proxy)
        if [ -n "${PUBLIC_PANEL_URL:-}" ]; then
            echo "  URL:     ${PUBLIC_PANEL_URL}"
        else
            echo "  URL:     http://$(hostname -I 2>/dev/null | awk '{print $1}' || echo 'server-ip'):18888 (via reverse proxy)"
        fi
        ;;
    *)
        echo "  URL:     http://$(hostname -I 2>/dev/null | awk '{print $1}' || echo 'server-ip'):18888"
        ;;
esac
echo "  DB:      ${RELAYPANEL_DB_MODE:-sqlite}"
echo "  Web:     $(web_mode_label "$RELAYPANEL_WEB_MODE")"
# Security: we deliberately do NOT print the default admin/admin123 credentials
# here, nor do we probe them (probing an unchanged default is a needless login
# attempt that can trip rate-limiting; printing it trains users to ignore creds
# in shell output). First-time installers are told once in the README to change
# the password; upgrades use whatever password the admin already set.
echo "  Login:   open the URL above and sign in"
echo ""
echo "  If this is a FIRST install, change the default admin password now."
if [ "${RELAYPANEL_WITH_NODE:-0}" != "1" ]; then
    echo ""
    echo "  Only the panel is running. To forward traffic, install relay-node"
    echo "  on a separate server via the panel's Device Groups page."
    echo "  To ALSO run node on this host: RELAYPANEL_WITH_NODE=1 ./deploy.sh"
fi
echo ""
echo "  Logs:      docker compose -f $COMPOSE_FILE ${PROFILE_ARGS[*]} logs -f"
echo "  Stop:      docker compose -f $COMPOSE_FILE ${PROFILE_ARGS[*]} down"
echo "  Update:    git pull --quiet && ./deploy.sh"
