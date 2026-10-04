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
        *' ps --all -q panel')
            [ -n "${HARNESS_PS_FAILS:-}" ] && exit 1
            [ -n "${HARNESS_DATA_DIR:-}" ] && echo 'panel123'; exit 0 ;;
        *' stop panel')
            printf 'STOP panel\n' >> "$log"
            [ -n "${HARNESS_STOP_FAILS:-}" ] && exit 1
            touch "$log.stopped"; exit 0 ;;
        *' start panel') printf 'START panel\n' >> "$log"; rm -f "$log.stopped"; exit 0 ;;
        *' config') printf 'name: harness\nservices: {}\n'; exit 0 ;;
        *' build'*) printf 'BUILD %s\n' "$*" >> "$log"; exit 0 ;;
        *' pull') printf 'PULL %s\n' "$*" >> "$log"; exit 0 ;;
        *' up -d'*)
            printf 'UP %s\n' "$*" >> "$log"
            [ -n "${HARNESS_UP_FAILS:-}" ] && { echo 'compose up exploded' >&2; exit 1; }
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
        *'Config.Env'*'panel123')
            printf '%s\n' "${HARNESS_CONTAINER_ENV:-DATABASE_URL=sqlite:/app/data/data.db?mode=rwc}"
            exit 0 ;;
        *'.State.Running'*'panel123')
            if [ -f "$log.stopped" ] || [ -n "${HARNESS_PANEL_STOPPED:-}" ]; then echo false; else echo true; fi
            exit 0 ;;
    esac
fi
if [ "${1:-}" = "volume" ]; then
    case "$*" in
        'volume ls'*'com.docker.compose.project=harness'*'com.docker.compose.volume=panel_data'*)
            [ -n "${HARNESS_VOLUME_DIR:-}" ] && echo 'harness_panel_data'; exit 0 ;;
        'volume inspect'*'harness_panel_data') echo "${HARNESS_VOLUME_DIR:-}"; exit 0 ;;
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
# v1.2.12: the panel refuses a short JWT_SECRET, so an upgrade must replace it.
assert_file_has "$dir/.env" 'JWT_SECRET=0123456789abcdef0123456789abcdef'
grep -q '^JWT_SECRET=x$' "$dir/.env" && fail 'short JWT_SECRET survived the upgrade'
pass 'upgrade replaces a short JWT_SECRET'

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
JWT_SECRET=strong-existing-secret-strong-existing-secret
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
assert_file_has "$dir/.env" 'JWT_SECRET=strong-existing-secret-strong-existing-secret'
pass 'upgrade keeps an existing strong JWT_SECRET'

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

# If the new version cannot be started after the backup stopped the old
# panel, deploy.sh must start the old container again and fail.
res=$(run_case upgrade-up-fails env)
dir=${res%|*}; log=${res#*|}
mkdir -p "$TMP/upgrade-up-fails-data"
echo 'sqlite bytes' > "$TMP/upgrade-up-fails-data/data.db"
: > "$log"
if (cd "$dir" && HARNESS_LOG="$log" HARNESS_DATA_DIR="$TMP/upgrade-up-fails-data" HARNESS_UP_FAILS=1 PATH="$TMP/fakebin-upgrade-up-fails:$PATH" bash ./deploy.sh >/tmp/rp-upgrade-up-fails-2.out 2>/tmp/rp-upgrade-up-fails-2.err); then
    fail 'deploy.sh must fail when compose up fails'
fi
grep -q '^STOP panel' "$log" || fail 'expected the backup to stop the panel'
grep -q '^START panel' "$log" || fail 'the old panel must be started again when compose up fails'
pass 'a failed compose up after the backup brings the old panel back'

# v1.2.12 (pre-release review): an admin who stopped the panel before
# upgrading still gets a backup — and the panel is not "stopped" again.
res=$(run_case upgrade-stopped-panel env)
dir=${res%|*}; log=${res#*|}
mkdir -p "$TMP/upgrade-stopped-panel-data"
echo 'sqlite bytes' > "$TMP/upgrade-stopped-panel-data/data.db"
: > "$log"
(cd "$dir" && HARNESS_LOG="$log" HARNESS_DATA_DIR="$TMP/upgrade-stopped-panel-data" HARNESS_PANEL_STOPPED=1 PATH="$TMP/fakebin-upgrade-stopped-panel:$PATH" bash ./deploy.sh >/tmp/rp-upgrade-stopped-panel-2.out 2>/tmp/rp-upgrade-stopped-panel-2.err) \
    || fail 'upgrade with a stopped panel failed'
ls "$dir"/backups/data-*-pre-deploy.db >/dev/null 2>&1 || fail 'a stopped panel must still be backed up'
grep -q '^STOP panel' "$log" && fail 'an already stopped panel must not be stopped again'
pass 'upgrade backs up an already stopped panel'

# A panel that cannot be stopped is not copied mid-write, and nothing changes.
res=$(run_case upgrade-stop-fails env)
dir=${res%|*}; log=${res#*|}
mkdir -p "$TMP/upgrade-stop-fails-data"
echo 'sqlite bytes' > "$TMP/upgrade-stop-fails-data/data.db"
: > "$log"
if (cd "$dir" && HARNESS_LOG="$log" HARNESS_DATA_DIR="$TMP/upgrade-stop-fails-data" HARNESS_STOP_FAILS=1 PATH="$TMP/fakebin-upgrade-stop-fails:$PATH" bash ./deploy.sh >/tmp/rp-upgrade-stop-fails-2.out 2>/tmp/rp-upgrade-stop-fails-2.err); then
    fail 'deploy.sh must stop when the panel cannot be stopped for the backup'
fi
ls "$dir"/backups/data-*.db >/dev/null 2>&1 && fail 'no backup may be taken while the panel may still write'
grep -q '^UP ' "$log" && fail 'the new version must not be started without a backup'
pass 'a panel that cannot be stopped blocks the upgrade'

# After `docker compose down` the container is gone but the volume is not.
res=$(run_case upgrade-after-down env)
dir=${res%|*}; log=${res#*|}
mkdir -p "$TMP/upgrade-after-down-volume"
echo 'sqlite bytes' > "$TMP/upgrade-after-down-volume/data.db"
: > "$log"
(cd "$dir" && HARNESS_LOG="$log" HARNESS_VOLUME_DIR="$TMP/upgrade-after-down-volume" PATH="$TMP/fakebin-upgrade-after-down:$PATH" bash ./deploy.sh >/tmp/rp-upgrade-after-down-2.out 2>/tmp/rp-upgrade-after-down-2.err) \
    || fail 'upgrade after compose down failed'
ls "$dir"/backups/data-*-pre-deploy.db >/dev/null 2>&1 || fail 'the database in the leftover volume must be backed up'
pass 'upgrade after compose down backs up the volume'

# A failed container lookup must not be taken for "no panel": no backup
# may be skipped, and above all no database copied without the stop check.
res=$(run_case upgrade-ps-fails env)
dir=${res%|*}; log=${res#*|}
mkdir -p "$TMP/upgrade-ps-fails-volume"
echo 'sqlite bytes' > "$TMP/upgrade-ps-fails-volume/data.db"
: > "$log"
if (cd "$dir" && HARNESS_LOG="$log" HARNESS_PS_FAILS=1 HARNESS_VOLUME_DIR="$TMP/upgrade-ps-fails-volume" PATH="$TMP/fakebin-upgrade-ps-fails:$PATH" bash ./deploy.sh >/tmp/rp-upgrade-ps-fails-2.out 2>/tmp/rp-upgrade-ps-fails-2.err); then
    fail 'deploy.sh must stop when the container lookup fails'
fi
ls "$dir"/backups/data-*.db >/dev/null 2>&1 && fail 'a failed lookup must not lead to a copy'
grep -q '^UP ' "$log" && fail 'the new version must not be started after a failed lookup'
pass 'a failed container lookup blocks the upgrade'

# v1.2.12 (full audit): the database is the file DATABASE_URL names, read
# from the running container; a custom name used to be skipped.
res=$(run_case upgrade-custom-db env)
dir=${res%|*}; log=${res#*|}
mkdir -p "$TMP/upgrade-custom-db-data"
echo 'relay bytes' > "$TMP/upgrade-custom-db-data/relay.db"
: > "$log"
(cd "$dir" && HARNESS_LOG="$log" HARNESS_DATA_DIR="$TMP/upgrade-custom-db-data" HARNESS_CONTAINER_ENV='DATABASE_URL=sqlite:/app/data/relay.db?mode=rwc' PATH="$TMP/fakebin-upgrade-custom-db:$PATH" bash ./deploy.sh >/tmp/rp-upgrade-custom-db-2.out 2>/tmp/rp-upgrade-custom-db-2.err) \
    || fail 'upgrade with a custom SQLite file name failed'
grep -q 'relay bytes' "$dir"/backups/data-*-pre-deploy.db 2>/dev/null \
    || fail 'the file DATABASE_URL names must be backed up'
pass 'upgrade backs up a SQLite file with a custom name'

# A database outside the data volume cannot be copied from the host: stop.
res=$(run_case upgrade-db-outside env)
dir=${res%|*}; log=${res#*|}
mkdir -p "$TMP/upgrade-db-outside-data"
echo 'sqlite bytes' > "$TMP/upgrade-db-outside-data/data.db"
: > "$log"
if (cd "$dir" && HARNESS_LOG="$log" HARNESS_DATA_DIR="$TMP/upgrade-db-outside-data" HARNESS_CONTAINER_ENV='DATABASE_URL=sqlite:/srv/relay.db' PATH="$TMP/fakebin-upgrade-db-outside:$PATH" bash ./deploy.sh >/tmp/rp-upgrade-db-outside-2.out 2>/tmp/rp-upgrade-db-outside-2.err); then
    fail 'deploy.sh must stop when the database is outside the data volume'
fi
grep -q '^UP ' "$log" && fail 'the new version must not start without a backup'
grep -q 'RELAYPANEL_BACKUP_DONE=1' /tmp/rp-upgrade-db-outside-2.err \
    || fail 'the message must say how to go ahead after a manual backup'
pass 'a database outside the data volume blocks the upgrade'

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
