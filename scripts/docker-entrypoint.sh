#!/bin/sh
# Panel container entrypoint (v1.2.12): start as root only long enough to fix
# ownership, then run the panel as the unprivileged `relaypanel` user.
#
# Earlier images ran the panel itself as root, so every existing /app/data
# volume and ./run bind mount is root-owned. Handing them over here — rather
# than just setting USER in the Dockerfile — is what lets an existing install
# upgrade in place: a non-root panel could not otherwise open its SQLite file.
#
#   /app/data  the SQLite database (a named volume or bind mount). Only entries
#              not yet owned by relaypanel are touched, so a normal restart
#              costs one directory walk. -h changes a symlink itself, never
#              what it points to.
#   /app/run   the one-click update's request/status directory (bind mount of
#              the host's ./run). The panel only needs to create files in it,
#              so only the mount point itself changes owner — not recursively,
#              since the host updater writes files there as root.
#
# Started with `--user` / `user:` (not root): nothing to fix, run as given.

set -eu

if [ "$(id -u)" = "0" ]; then
    if [ -d /app/data ]; then
        find /app/data ! -user relaypanel -exec chown -h relaypanel:relaypanel {} +
    fi
    if [ -d /app/run ]; then
        chown relaypanel:relaypanel /app/run
    fi
    exec setpriv --reuid=relaypanel --regid=relaypanel --init-groups -- "$@"
fi

exec "$@"
