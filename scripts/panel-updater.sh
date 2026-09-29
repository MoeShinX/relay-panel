#!/usr/bin/env bash
# RelayPanel one-click update — the host's half (v1.2.11).
#
# Run by relaypanel-updater.service (installed by deploy.sh) when the panel
# drops ./run/update-request. It does what an operator does by hand:
#
#     git pull --ff-only && ./deploy.sh
#
# with a database backup in between for SQLite, and it reports the outcome in
# ./run/update-status.json for the panel to show. The panel's half, and why the
# panel cannot simply update itself, is in crates/panel/src/api/panel_update.rs.
#
# TRUST BOUNDARY — read before editing.
# ./run is bind-mounted into the panel container, so the panel can write
# anything there. This script runs as root. Everything in ./run is therefore
# untrusted input:
#   - Nothing in ./run is executed or sourced, and the request file's CONTENT is
#     never read: the version installed is whatever the official repository's
#     main branch says, not what the panel asked for.
#   - Files are written into ./run only as a fresh mktemp file renamed into
#     place. A rename replaces a planted symlink instead of following it, so a
#     compromised panel cannot point update-status.json at /etc/shadow and have
#     root overwrite it.
#   - The full log goes to a host-only path; only a sanitised tail is copied in.

set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
RUN_DIR="$ROOT/run"
# Overridable only so scripts/test-panel-updater.sh can run this unprivileged.
LOG="${RELAYPANEL_UPDATER_LOG:-/var/log/relaypanel-updater.log}"
LOCK="${RELAYPANEL_UPDATER_LOCK:-/run/relaypanel-updater.lock}"
PANEL_HEALTH=http://127.0.0.1:18888/api/v1/health

cd "$ROOT" || exit 1

# One run at a time. A second request while this one works is dropped: the
# panel refuses to write one while a run is in progress anyway.
exec 9>"$LOCK"
if ! flock -n 9; then
    echo "another update is already running" >&2
    exit 0
fi

# Consume the request first. The .path unit watches for its existence; leaving
# it in place would re-trigger this service the moment it exits.
#
# -r because the panel can create anything there — a directory named
# update-request would survive `rm -f` and turn the path unit into a loop of
# git pulls and deploys. rm does not follow symlinks, so -r is safe here. If it
# still cannot be removed, stop rather than run in that loop.
rm -rf -- "$RUN_DIR/update-request"
if [ -e "$RUN_DIR/update-request" ] || [ -L "$RUN_DIR/update-request" ]; then
    echo "cannot remove $RUN_DIR/update-request; refusing to run" >&2
    exit 1
fi

STARTED_AT=$(date +%s)
: > "$LOG.run"

log() { echo "[$(date '+%F %T')] $*" | tee -a "$LOG.run" >&2; }

# Read the running panel's version from its public health endpoint.
panel_version() {
    curl -fsS --max-time 5 "$PANEL_HEALTH" 2>/dev/null \
        | grep -o '"version"[[:space:]]*:[[:space:]]*"[^"]*"' \
        | head -1 | sed 's/.*"\([^"]*\)"$/\1/'
}

# stdin -> a JSON string literal. Strips ANSI colour codes and every control
# character except newline (docker's progress output is full of both, and a
# raw control character makes the whole file unparseable).
json_str() {
    sed 's/\x1b\[[0-9;?]*[A-Za-z]//g' \
        | LC_ALL=C tr -d '\000-\010\013-\037' \
        | awk 'BEGIN { ORS=""; print "\"" }
               { gsub(/\\/, "\\\\"); gsub(/"/, "\\\""); gsub(/\t/, "\\t");
                 if (NR > 1) print "\\n"; print }
               END { print "\"" }'
}

# write_status STATE MESSAGE [TO_VERSION]
write_status() {
    local state="$1" message="$2" to="${3:-}" finished=0 tmp
    [ "$state" = "running" ] || finished=$(date +%s)
    tmp=$(mktemp "$RUN_DIR/.status.XXXXXX") || return 0
    {
        printf '{"state":%s,' "$(printf '%s' "$state" | json_str)"
        printf '"from_version":%s,' "$(printf '%s' "$FROM" | json_str)"
        printf '"to_version":%s,' "$(printf '%s' "$to" | json_str)"
        printf '"started_at":%s,"finished_at":%s,' "$STARTED_AT" "$finished"
        printf '"message":%s,' "$(printf '%s' "$message" | json_str)"
        printf '"log_tail":%s}\n' "$(tail -n 30 "$LOG.run" | json_str)"
    } > "$tmp"
    chmod 644 "$tmp"
    mv -f "$tmp" "$RUN_DIR/update-status.json"
}

finish() { # STATE MESSAGE [TO]
    write_status "$@"
    cat "$LOG.run" >> "$LOG"
    rm -f "$LOG.run"
    exit 0
}

env_get() { # KEY -> value from .env (last assignment wins), unquoted
    [ -f .env ] || return 0
    grep -E "^$1=" .env | tail -1 | cut -d= -f2- | sed 's/^["'\'']//; s/["'\'']$//'
}

FROM="$(panel_version)"
log "update requested; running panel version: ${FROM:-unknown}"
write_status running "Updating"

# A pinned tag would make "update" a restart onto the same version. Say so
# instead of pretending.
PINNED="$(env_get RELAYPANEL_PANEL_TAG)"
if [ -n "$PINNED" ]; then
    log "RELAYPANEL_PANEL_TAG=$PINNED is pinned in .env"
    finish pinned "RELAYPANEL_PANEL_TAG=$PINNED is pinned in .env. Remove it (or change it) to update."
fi

# ---- 1. Fetch the new release definition ----
log "git pull --ff-only"
# safe.directory: this runs as root, and git refuses to touch a repository
# owned by another user (a clone made as a regular user, deployed with sudo).
if ! git -c safe.directory="$ROOT" pull --ff-only --quiet >>"$LOG.run" 2>&1; then
    finish failed "git pull failed: GitHub unreachable from this server, or local changes in $ROOT (see the log below). Nothing was changed."
fi

RELEASE_COMPOSE="docker-compose.release.yaml"
TARGET="$(grep -o 'relay-panel-panel:\${RELAYPANEL_PANEL_TAG:-[^}]*}' "$RELEASE_COMPOSE" 2>/dev/null \
    | head -1 | sed 's/.*:-\([^}]*\)}/\1/')"
log "release compose now pins panel ${TARGET:-unknown}"
if [ -n "$TARGET" ] && [ "$TARGET" = "$FROM" ]; then
    finish up_to_date "Already on $FROM." "$FROM"
fi

# ---- 2. Download the new images while the old panel keeps serving ----
if [ "$(env_get RELAYPANEL_BUILD_LOCAL)" != "1" ]; then
    log "pre-pulling images"
    docker compose -f "$RELEASE_COMPOSE" pull panel >>"$LOG.run" 2>&1 \
        || log "pre-pull failed; deploy.sh will try again"
fi

# ---- 3. Back up the database (SQLite) ----
# scripts/sqlite-backup.sh stops the panel, copies the database and leaves the
# panel stopped for deploy.sh to start the new one. v1.2.12: deploy.sh uses the
# same script for manual upgrades; RELAYPANEL_BACKUP_DONE below tells it this
# run already has its backup.
DB_MODE="$(env_get RELAYPANEL_DB_MODE)"
BACKUP_NOTE=""
if [ -z "$DB_MODE" ] || [ "$DB_MODE" = "sqlite" ]; then
    backup="$(bash "$ROOT/scripts/sqlite-backup.sh" "$RELEASE_COMPOSE" "v${FROM:-unknown}" 2>>"$LOG.run")"
    case $? in
        0)
            log "backed up to $backup"
            BACKUP_NOTE=" Database backed up to backups/${backup##*/}."
            ;;
        3)
            log "could not locate the SQLite database; continuing without a backup"
            BACKUP_NOTE=" Database NOT backed up (not found)."
            ;;
        *)
            # Refuse to continue without a backup: the new version may migrate
            # the schema, and this is the one moment to keep a way back. The
            # backup script already started the old panel again.
            finish failed "Database backup failed, so the update was not applied. The panel was restarted on the old version."
            ;;
    esac
else
    BACKUP_NOTE=" PostgreSQL is not backed up automatically."
fi

# ---- 4. Deploy ----
# stdin from /dev/null: an upgrade never prompts, and if something ever tried
# to, it must fail here rather than wait forever with no terminal.
log "running deploy.sh"
if RELAYPANEL_BACKUP_DONE=1 bash ./deploy.sh </dev/null >>"$LOG.run" 2>&1; then
    TO="$(panel_version)"
    log "panel is up on ${TO:-unknown}"
    finish succeeded "Updated ${FROM:-?} → ${TO:-?}.$BACKUP_NOTE" "$TO"
fi

# deploy.sh failed. If we stopped the panel, make sure SOMETHING is serving.
log "deploy.sh failed"
docker compose -f "$RELEASE_COMPOSE" up -d panel >>"$LOG.run" 2>&1 || true
NOW="$(panel_version)"
finish failed "deploy.sh failed.${NOW:+ The panel is running $NOW.}$BACKUP_NOTE Full log: $LOG" "$NOW"
