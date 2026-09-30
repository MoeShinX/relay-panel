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
# inconsistent. This makes sure the panel is stopped, copies data.db with its
# -wal/-shm files to backups/data-<timestamp>-<label>.db and keeps the newest
# $KEEP_BACKUPS.
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
# Whether data.db exists can only be told from a readable directory: a named
# volume's host directory is root-only, and `-f` just says "no" when run
# without the rights to look.
if [ ! -d "$data_dir" ] || [ ! -r "$data_dir" ] || [ ! -x "$data_dir" ]; then
    lookup_failed "cannot read $data_dir (run as root — sudo ./deploy.sh — or back the database up yourself and re-run with RELAYPANEL_BACKUP_DONE=1)"
fi
if [ ! -f "$data_dir/data.db" ]; then
    echo "no data.db in $data_dir; nothing to back up" >&2
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
for f in data.db data.db-wal data.db-shm; do
    [ -f "$data_dir/$f" ] || continue
    cp -p "$data_dir/$f" "$BACKUP_DIR/${f/data.db/data-$stamp.db}" || ok=0
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

echo "backed up to $BACKUP_DIR/data-$stamp.db" >&2
echo "$BACKUP_DIR/data-$stamp.db"
