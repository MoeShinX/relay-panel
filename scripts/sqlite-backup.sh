#!/usr/bin/env bash
# Back up the panel's SQLite database before an upgrade (v1.2.12).
#
# Shared by deploy.sh (manual `git pull && ./deploy.sh`) and panel-updater.sh
# (one-click update), so both paths keep the same way back if a new version's
# schema migration goes wrong.
#
#   scripts/sqlite-backup.sh <compose-file> <label>
#
# SQLite runs in WAL mode, so a copy taken while the panel writes can be
# inconsistent. This makes sure the panel is stopped, copies the database file
# (the one DATABASE_URL names — data.db by default) with its -wal/-shm files to
# backups/data-<timestamp>-<label>.db and keeps the newest $KEEP_BACKUPS.
#
# Exit status:
#   0  backed up; the path is printed on stdout. The panel is left STOPPED
#      (stopped here if it was running) — the caller is about to start the new
#      version.
#   3  nothing to back up: every lookup succeeded and found no database; the
#      panel was not touched.
#   1  no backup: a lookup failed (so there may well be a database), the panel
#      could not be confirmed stopped, or the copy failed. A panel stopped here
#      was started again; nothing else changed. Callers must not go on to
#      upgrade: this is the one moment to keep a way back.
# Progress goes to stderr.

set -uo pipefail

COMPOSE_FILE="${1:?usage: sqlite-backup.sh <compose-file> <label>}"
LABEL="${2:?usage: sqlite-backup.sh <compose-file> <label>}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BACKUP_DIR="$ROOT/backups"
KEEP_BACKUPS=5

cd "$ROOT" || exit 1

# ---- Find the database ----
# Every query here must SUCCEED. A failed query is not "nothing there":
# taking it as such skipped the backup — or, with the container list failing,
# copied a database that might be in use — and the upgrade still went ahead.
# Only lookups that succeed and find nothing count as "no database" (exit 3).
lookup_failed() {
    echo "$1; not upgrading without a backup" >&2
    exit 1
}

# `ps --all`: a stopped panel (stopped by the admin before upgrading, or
# crashed) still has its database; plain `ps` lists only running containers,
# and the backup used to be skipped as "nothing to back up".
ids="$(docker compose -f "$COMPOSE_FILE" ps --all -q panel 2>/dev/null)" \
    || lookup_failed "could not list the panel containers"
cid="$(printf '%s\n' "$ids" | head -n1)"

# Which file is the database: the one the panel opens, DATABASE_URL. Read it
# from the panel container when there is one — that is what the version being
# replaced runs with — else take what compose will hand the new one: the
# environment, then .env, then the compose file's default. A file name other
# than data.db used to be taken for "no data.db, nothing to back up", and the
# upgrade went ahead without a backup.
db_url=""
if [ -n "$cid" ]; then
    env_lines="$(docker inspect -f '{{range .Config.Env}}{{println .}}{{end}}' "$cid" 2>/dev/null)" \
        || lookup_failed "could not read the panel container's environment"
    db_url="$(printf '%s\n' "$env_lines" | sed -n 's/^DATABASE_URL=//p' | tail -n1)"
fi
[ -n "$db_url" ] || db_url="${DATABASE_URL:-}"
if [ -z "$db_url" ] && [ -f .env ]; then
    # Read like deploy.sh's env_get: the last line wins, one pair of quotes goes.
    db_url="$(sed -n 's/^DATABASE_URL=//p' .env | tail -n1)"
    case "$db_url" in
        \"*\") db_url="${db_url#\"}"; db_url="${db_url%\"}" ;;
        \'*\') db_url="${db_url#\'}"; db_url="${db_url%\'}" ;;
    esac
fi
db_url="${db_url:-sqlite:/app/data/data.db?mode=rwc}"
case "$db_url" in
    postgres://*|postgresql://*)
        echo "the panel uses PostgreSQL; no SQLite database to back up" >&2
        exit 3
        ;;
esac
# Read the URL the way the panel (sqlx) does: drop the scheme, then the
# ?parameters. A relative path is relative to the panel's working dir, /app.
db_path="${db_url#sqlite://}"
db_path="${db_path#sqlite:}"
db_params=""
case "$db_path" in *\?*) db_params="${db_path#*\?}"; db_path="${db_path%%\?*}" ;; esac
if [ "$db_path" = ":memory:" ] || [[ "&$db_params&" == *"&mode=memory&"* ]]; then
    echo "the panel's SQLite database is in memory; nothing to back up" >&2
    exit 3
fi
case "$db_path" in /*) ;; *) db_path="/app/$db_path" ;; esac
db_file=""
case "$db_path" in /app/data/*) db_file="${db_path#/app/data/}" ;; esac
# Refused rather than skipped: outside the data volume there is no host copy
# to take, and a name this cannot read safely (.., %-escapes) is not guessed.
case "$db_file" in
    "" | */ | *..* | *%*)
        lookup_failed "the SQLite database $db_path is not a file in the panel's data volume (/app/data); back it up yourself and re-run with RELAYPANEL_BACKUP_DONE=1"
        ;;
esac

data_dir=""
if [ -n "$cid" ]; then
    data_dir="$(docker inspect -f \
        '{{range .Mounts}}{{if eq .Destination "/app/data"}}{{.Source}}{{end}}{{end}}' "$cid" 2>/dev/null)" \
        || lookup_failed "could not read the panel container's mounts"
else
    # No panel container at all (e.g. after `docker compose down`): the
    # default named volume outlives it. Find it by the labels compose puts on
    # this project's volumes.
    config="$(docker compose -f "$COMPOSE_FILE" config 2>/dev/null)" \
        || lookup_failed "could not read the compose configuration"
    project="$(printf '%s\n' "$config" | sed -n 's/^name: *//p' | head -n1)"
    [ -n "$project" ] || lookup_failed "could not tell the compose project name"
    vols="$(docker volume ls -q \
        --filter "label=com.docker.compose.project=$project" \
        --filter "label=com.docker.compose.volume=panel_data" 2>/dev/null)" \
        || lookup_failed "could not list the docker volumes"
    vol="$(printf '%s\n' "$vols" | head -n1)"
    if [ -n "$vol" ]; then
        data_dir="$(docker volume inspect -f '{{.Mountpoint}}' "$vol" 2>/dev/null)" \
            || lookup_failed "could not read the volume $vol"
    fi
fi
if [ -z "$data_dir" ]; then
    echo "no panel data volume; nothing to back up" >&2
    exit 3
fi
# Whether the database exists can only be told from a readable directory: a
# named volume's host directory is root-only, and `-f` just says "no" when run
# without the rights to look.
if [ ! -d "$data_dir" ] || [ ! -r "$data_dir" ] || [ ! -x "$data_dir" ]; then
    lookup_failed "cannot read $data_dir (run as root — sudo ./deploy.sh — or back the database up yourself and re-run with RELAYPANEL_BACKUP_DONE=1)"
fi
if [ ! -f "$data_dir/$db_file" ]; then
    echo "no $db_file in $data_dir; nothing to back up" >&2
    exit 3
fi

# ---- Make sure nothing is writing ----
# Copy only once the panel is confirmed stopped. If `stop` fails, or the state
# cannot be read, a copy taken while it writes may be inconsistent — and the
# caller would upgrade trusting it — so refuse instead.
is_running() {
    docker inspect -f '{{.State.Running}}' "$cid" 2>/dev/null
}
stopped_here=0
if [ -n "$cid" ]; then
    case "$(is_running)" in
        false) ;; # already stopped: leave it that way
        true)
            echo "stopping the panel for a consistent database backup" >&2
            if ! docker compose -f "$COMPOSE_FILE" stop panel >&2 || [ "$(is_running)" != "false" ]; then
                echo "could not confirm the panel stopped; not taking a possibly inconsistent backup" >&2
                docker compose -f "$COMPOSE_FILE" start panel >&2 || true
                exit 1
            fi
            stopped_here=1
            ;;
        *)
            echo "could not tell whether the panel is running; not taking a possibly inconsistent backup" >&2
            exit 1
            ;;
    esac
fi

# ---- Copy ----
mkdir -p "$BACKUP_DIR" && chmod 700 "$BACKUP_DIR"
stamp="$(date +%Y%m%d-%H%M%S)-$LABEL"
ok=1
for suffix in "" -wal -shm; do
    [ -f "$data_dir/$db_file$suffix" ] || continue
    cp -p "$data_dir/$db_file$suffix" "$BACKUP_DIR/data-$stamp.db$suffix" || ok=0
done
if [ "$ok" != "1" ]; then
    echo "database backup failed" >&2
    [ "$stopped_here" = "1" ] && docker compose -f "$COMPOSE_FILE" start panel >&2
    exit 1
fi

# Keep the newest $KEEP_BACKUPS.
# shellcheck disable=SC2012 # names are ours: data-<digits>-<label>.db
ls -1t "$BACKUP_DIR"/data-*.db 2>/dev/null | tail -n +$((KEEP_BACKUPS + 1)) \
    | while read -r old; do rm -f "$old" "$old-wal" "$old-shm"; done

echo "backed up $db_file to $BACKUP_DIR/data-$stamp.db" >&2
echo "$BACKUP_DIR/data-$stamp.db"
