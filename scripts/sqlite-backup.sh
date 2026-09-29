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
# inconsistent. This stops the panel, copies data.db with its -wal/-shm files to
# backups/data-<timestamp>-<label>.db and keeps the newest $KEEP_BACKUPS.
#
# Exit status:
#   0  backed up; the path is printed on stdout. The panel is left STOPPED —
#      the caller is about to start the new version.
#   3  nothing to back up (no panel container, or no data.db in its /app/data
#      mount); the panel was not touched.
#   1  the copy failed; the panel was started again and nothing else changed.
#      Callers must not go on to upgrade: this is the one moment to keep a way
#      back.
# Progress goes to stderr.

set -uo pipefail

COMPOSE_FILE="${1:?usage: sqlite-backup.sh <compose-file> <label>}"
LABEL="${2:?usage: sqlite-backup.sh <compose-file> <label>}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BACKUP_DIR="$ROOT/backups"
KEEP_BACKUPS=5

cd "$ROOT" || exit 1

cid="$(docker compose -f "$COMPOSE_FILE" ps -q panel 2>/dev/null)"
data_dir=""
[ -n "$cid" ] && data_dir="$(docker inspect -f \
    '{{range .Mounts}}{{if eq .Destination "/app/data"}}{{.Source}}{{end}}{{end}}' "$cid" 2>/dev/null)"
if [ -z "$data_dir" ] || [ ! -f "$data_dir/data.db" ]; then
    echo "could not locate the SQLite database; nothing to back up" >&2
    exit 3
fi

echo "stopping the panel for a consistent database backup" >&2
stopped=0
docker compose -f "$COMPOSE_FILE" stop panel >&2 && stopped=1
mkdir -p "$BACKUP_DIR" && chmod 700 "$BACKUP_DIR"
stamp="$(date +%Y%m%d-%H%M%S)-$LABEL"
ok=1
for f in data.db data.db-wal data.db-shm; do
    [ -f "$data_dir/$f" ] || continue
    cp -p "$data_dir/$f" "$BACKUP_DIR/${f/data.db/data-$stamp.db}" || ok=0
done
if [ "$ok" != "1" ]; then
    echo "database backup failed" >&2
    [ "$stopped" = "1" ] && docker compose -f "$COMPOSE_FILE" start panel >&2
    exit 1
fi

# Keep the newest $KEEP_BACKUPS.
# shellcheck disable=SC2012 # names are ours: data-<digits>-<label>.db
ls -1t "$BACKUP_DIR"/data-*.db 2>/dev/null | tail -n +$((KEEP_BACKUPS + 1)) \
    | while read -r old; do rm -f "$old" "$old-wal" "$old-shm"; done

echo "backed up to $BACKUP_DIR/data-$stamp.db" >&2
echo "$BACKUP_DIR/data-$stamp.db"
