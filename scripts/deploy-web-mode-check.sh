#!/usr/bin/env bash
#
# Offline deploy.sh web-mode harness. It stubs docker/curl/openssl/ss so the
# deployment-mode branches can be tested without a Docker daemon.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

fail() { echo "[FAIL] $*" >&2; exit 1; }
pass() { echo "[OK] $*"; }

make_fakebin() {
    local dir="$1"
    mkdir -p "$dir"
    cat > "$dir/openssl" <<'SH'
#!/usr/bin/env bash
printf '0123456789abcdef0123456789abcdef\n'
SH
    cat > "$dir/ss" <<'SH'
#!/usr/bin/env bash
exit 0
SH
    cat > "$dir/curl" <<'SH'
#!/usr/bin/env bash
out="" headers=""
while [ $# -gt 0 ]; do
    case "$1" in
        -o) out="$2"; shift 2 ;;
        -D) headers="$2"; shift 2 ;;
        -w) shift 2 ;;
        -*) shift ;;
        *) url="$1"; shift ;;
    esac
done
case "${url:-}" in
    *'/api/v1/health')
        [ -n "$out" ] && printf '{"status":"ok","version":"0.4.8"}' > "$out"
        [ -n "$headers" ] && printf 'content-type: application/json\n' > "$headers"
        printf '200'
        ;;
    https://*)
        printf 'CADDY_HTTPS %s\n' "$url" >> "${HARNESS_LOG:?HARNESS_LOG not set}"
        [ -n "$out" ] && : > "$out"
        exit 0
        ;;
    *)
        [ -n "$out" ] && : > "$out"
        exit 0
        ;;
esac
SH
    cat > "$dir/docker" <<'SH'
#!/usr/bin/env bash
log="${HARNESS_LOG:?HARNESS_LOG not set}"
case "$*" in
    '--version') echo 'Docker version 27.0.0'; exit 0 ;;
    'compose version') echo 'Docker Compose version v2.27.0'; exit 0 ;;
esac
if [ "${1:-}" = "compose" ]; then
    shift
    case "$*" in
        *' ps -q caddy') echo 'caddy123'; exit 0 ;;
        *' ps -q postgres') echo 'pg123'; exit 0 ;;
        *' ps -q panel') [ -n "${HARNESS_DATA_DIR:-}" ] && echo 'panel123'; exit 0 ;;
        *' stop panel') printf 'STOP panel\n' >> "$log"; exit 0 ;;
        *' start panel') printf 'START panel\n' >> "$log"; exit 0 ;;
        *' build'*) printf 'BUILD %s\n' "$*" >> "$log"; exit 0 ;;
        *' pull') printf 'PULL %s\n' "$*" >> "$log"; exit 0 ;;
        *' up -d'*)
            printf 'UP %s\n' "$*" >> "$log"
            env | grep -E '^(RELAYPANEL_WEB_MODE|RELAYPANEL_PANEL_PORT_BINDING|RELAYPANEL_DOMAIN|PUBLIC_PANEL_URL|REVERSE_PROXY_EXTERNAL|ACME_EMAIL|CADDY_ACME_EMAIL_DIRECTIVE|RELAYPANEL_DB_MODE)=' | sort >> "$log"
            exit 0
            ;;
    esac
fi
if [ "${1:-}" = "inspect" ]; then
    case "$*" in
        *'.State.Health.Status'*'pg123') echo 'healthy'; exit 0 ;;
        *'.State.Status'*'caddy123') echo 'running'; exit 0 ;;
        *'/app/data'*'panel123') echo "${HARNESS_DATA_DIR:-}"; exit 0 ;;
    esac
fi
echo "unexpected docker args: $*" >> "$log"
exit 0
SH
    chmod +x "$dir"/*
}

make_case_dir() {
    local dir="$1"
    mkdir -p "$dir"
    cp "$ROOT/deploy.sh" "$ROOT/docker-compose.release.yaml" "$ROOT/docker-compose.yaml" "$ROOT/Caddyfile" "$dir/"
    mkdir -p "$dir/scripts"
    cp "$ROOT/scripts/sqlite-backup.sh" "$dir/scripts/"
}

assert_file_has() {
    local file="$1" needle="$2"
    grep -Fq "$needle" "$file" || fail "$file missing: $needle"
}

assert_log_has() {
    local log="$1" needle="$2"
    grep -Fq -- "$needle" "$log" || fail "$log missing: $needle"
}

run_case() {
    local name="$1"
    shift
    local dir="$TMP/$name" fake="$TMP/fakebin-$name" log="$TMP/$name.log"
    make_case_dir "$dir"
    make_fakebin "$fake"
    : > "$log"
    (cd "$dir" && HARNESS_LOG="$log" PATH="$fake:$PATH" "$@" bash ./deploy.sh >"/tmp/rp-${name}.out" 2>"/tmp/rp-${name}.err") || {
        cat "/tmp/rp-${name}.out" >&2 || true
        cat "/tmp/rp-${name}.err" >&2 || true
        fail "$name failed"
    }
    echo "$dir|$log"
}

# Fresh direct: non-interactive default remains direct and public.
res=$(run_case fresh-direct env)
dir=${res%|*}; log=${res#*|}
assert_file_has "$dir/.env" 'RELAYPANEL_WEB_MODE=direct'
assert_log_has "$log" 'RELAYPANEL_PANEL_PORT_BINDING=0.0.0.0:18888'
pass 'fresh direct mode persists and binds public port'

# Env-selected Caddy must persist mode/domain/public URL and use localhost panel.
res=$(run_case env-caddy env RELAYPANEL_WEB_MODE=caddy RELAYPANEL_DOMAIN=panel.example.com ACME_EMAIL=admin@example.com)
dir=${res%|*}; log=${res#*|}
assert_file_has "$dir/.env" 'RELAYPANEL_WEB_MODE=caddy'
assert_file_has "$dir/.env" 'RELAYPANEL_DOMAIN=panel.example.com'
assert_file_has "$dir/.env" 'PUBLIC_PANEL_URL=https://panel.example.com'
assert_file_has "$dir/.env" 'ACME_EMAIL=admin@example.com'
assert_log_has "$log" 'CADDY_ACME_EMAIL_DIRECTIVE=email admin@example.com'
assert_log_has "$log" '--profile caddy'
assert_log_has "$log" 'RELAYPANEL_PANEL_PORT_BINDING=127.0.0.1:18888'
assert_log_has "$log" 'CADDY_HTTPS https://panel.example.com/'
pass 'env-selected Caddy persists and enables caddy profile'

# Env-selected separate-host reverse proxy must persist REVERSE_PROXY_EXTERNAL.
res=$(run_case env-rp-external env RELAYPANEL_WEB_MODE=reverse-proxy REVERSE_PROXY_EXTERNAL=1 PUBLIC_PANEL_URL=https://env-rp.example.com)
dir=${res%|*}; log=${res#*|}
assert_file_has "$dir/.env" 'RELAYPANEL_WEB_MODE=reverse-proxy'
assert_file_has "$dir/.env" 'REVERSE_PROXY_EXTERNAL=1'
assert_file_has "$dir/.env" 'PUBLIC_PANEL_URL=https://env-rp.example.com'
assert_log_has "$log" 'RELAYPANEL_PANEL_PORT_BINDING=0.0.0.0:18888'
pass 'env-selected separate-host reverse-proxy persists external binding'

# Upgrade same-host reverse proxy keeps PUBLIC_PANEL_URL and localhost binding.
res=$(run_case upgrade-rp-same env)
dir=${res%|*}; log=${res#*|}
cat > "$dir/.env" <<ENV
JWT_SECRET=x
PANEL_KEY=y
DATABASE_URL=sqlite:/app/data/data.db?mode=rwc
RELAYPANEL_WEB_MODE=reverse-proxy
PUBLIC_PANEL_URL=https://rp.example.com
ENV
(cd "$dir" && HARNESS_LOG="$log" PATH="$TMP/fakebin-upgrade-rp-same:$PATH" bash ./deploy.sh >/tmp/rp-upgrade-rp-same-2.out 2>/tmp/rp-upgrade-rp-same-2.err)
assert_file_has "$dir/.env" 'PUBLIC_PANEL_URL=https://rp.example.com'
assert_log_has "$log" 'RELAYPANEL_PANEL_PORT_BINDING=127.0.0.1:18888'
pass 'upgrade reverse-proxy preserves PUBLIC_PANEL_URL and same-host binding'

# Upgrade separate-host reverse proxy uses public panel bind.
res=$(run_case upgrade-rp-external env)
dir=${res%|*}; log=${res#*|}
cat > "$dir/.env" <<ENV
JWT_SECRET=x
PANEL_KEY=y
DATABASE_URL=sqlite:/app/data/data.db?mode=rwc
RELAYPANEL_WEB_MODE=reverse-proxy
REVERSE_PROXY_EXTERNAL=1
PUBLIC_PANEL_URL=https://rp-ext.example.com
ENV
(cd "$dir" && HARNESS_LOG="$log" PATH="$TMP/fakebin-upgrade-rp-external:$PATH" bash ./deploy.sh >/tmp/rp-upgrade-rp-external-2.out 2>/tmp/rp-upgrade-rp-external-2.err)
assert_log_has "$log" 'RELAYPANEL_PANEL_PORT_BINDING=0.0.0.0:18888'
pass 'separate-host reverse-proxy binds public port explicitly'

# Embedded PostgreSQL plus Caddy enables both profiles.
res=$(run_case pg-caddy env)
dir=${res%|*}; log=${res#*|}
cat > "$dir/.env" <<ENV
JWT_SECRET=x
PANEL_KEY=y
DATABASE_URL=postgres://relaypanel:pass@postgres:5432/relaypanel
RELAYPANEL_DB_MODE=embedded-postgres
RELAYPANEL_WEB_MODE=caddy
RELAYPANEL_DOMAIN=pgcaddy.example.com
PUBLIC_PANEL_URL=https://pgcaddy.example.com
ENV
(cd "$dir" && HARNESS_LOG="$log" PATH="$TMP/fakebin-pg-caddy:$PATH" bash ./deploy.sh >/tmp/rp-pg-caddy-2.out 2>/tmp/rp-pg-caddy-2.err)
assert_log_has "$log" '--profile postgres --profile caddy'
assert_log_has "$log" 'RELAYPANEL_PANEL_PORT_BINDING=127.0.0.1:18888'
assert_log_has "$log" 'CADDY_HTTPS https://pgcaddy.example.com/'
pass 'embedded PostgreSQL and Caddy profiles compose together'

# v1.2.12: a manual SQLite upgrade backs the database up with the panel
# stopped, before the new version starts.
res=$(run_case upgrade-backup env)
dir=${res%|*}; log=${res#*|}
mkdir -p "$TMP/upgrade-backup-data"
echo 'sqlite bytes' > "$TMP/upgrade-backup-data/data.db"
: > "$log"
(cd "$dir" && HARNESS_LOG="$log" HARNESS_DATA_DIR="$TMP/upgrade-backup-data" PATH="$TMP/fakebin-upgrade-backup:$PATH" bash ./deploy.sh >/tmp/rp-upgrade-backup-2.out 2>/tmp/rp-upgrade-backup-2.err) \
    || fail 'upgrade with a SQLite database failed'
ls "$dir"/backups/data-*-pre-deploy.db >/dev/null 2>&1 || fail 'upgrade did not back up the SQLite database'
stop_line=$(grep -n '^STOP panel' "$log" | head -1 | cut -d: -f1)
up_line=$(grep -n '^UP ' "$log" | head -1 | cut -d: -f1)
[ -n "$stop_line" ] && [ -n "$up_line" ] && [ "$stop_line" -lt "$up_line" ] \
    || fail 'the panel must be stopped for the copy before the new version starts'
pass 'manual upgrade backs up SQLite before starting the new version'

# The one-click updater has already taken its backup and says so.
res=$(run_case upgrade-backup-done env)
dir=${res%|*}; log=${res#*|}
mkdir -p "$TMP/upgrade-backup-done-data"
echo 'sqlite bytes' > "$TMP/upgrade-backup-done-data/data.db"
: > "$log"
(cd "$dir" && HARNESS_LOG="$log" HARNESS_DATA_DIR="$TMP/upgrade-backup-done-data" RELAYPANEL_BACKUP_DONE=1 PATH="$TMP/fakebin-upgrade-backup-done:$PATH" bash ./deploy.sh >/tmp/rp-upgrade-backup-done-2.out 2>/tmp/rp-upgrade-backup-done-2.err) \
    || fail 'upgrade with RELAYPANEL_BACKUP_DONE=1 failed'
[ ! -d "$dir/backups" ] || fail 'RELAYPANEL_BACKUP_DONE=1 must skip the second backup'
grep -q '^STOP panel' "$log" && fail 'RELAYPANEL_BACKUP_DONE=1 must not stop the panel'
pass 'upgrade skips the backup when the updater already took one'

# A fresh install has nothing to back up.
res=$(run_case fresh-no-backup env)
dir=${res%|*}
[ ! -d "$dir/backups" ] || fail 'a fresh install must not create backups'
pass 'fresh install takes no backup'

# Invalid Caddy domain must fail before compose starts.
dir="$TMP/bad-domain"; fake="$TMP/fakebin-bad-domain"; log="$TMP/bad-domain.log"
make_case_dir "$dir"; make_fakebin "$fake"; : > "$log"
if (cd "$dir" && HARNESS_LOG="$log" PATH="$fake:$PATH" env RELAYPANEL_WEB_MODE=caddy RELAYPANEL_DOMAIN=https://bad.example.com bash ./deploy.sh >/tmp/rp-bad-domain.out 2>/tmp/rp-bad-domain.err); then
    fail 'invalid Caddy domain unexpectedly succeeded'
fi
pass 'invalid Caddy domain is rejected'

pass 'deploy web-mode harness completed'
