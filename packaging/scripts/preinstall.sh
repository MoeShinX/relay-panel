#!/bin/sh
set -eu

if ! getent group relaypanel >/dev/null 2>&1; then
    groupadd --system relaypanel
fi

if ! id -u relaypanel >/dev/null 2>&1; then
    NOLOGIN="$(command -v nologin || printf '%s' /usr/sbin/nologin)"
    useradd --system --gid relaypanel --home-dir /var/lib/relay-panel \
        --shell "$NOLOGIN" --no-create-home relaypanel
fi

install -d -o relaypanel -g relaypanel -m 0750 /var/lib/relay-panel
install -d -o root -g relaypanel -m 0750 /etc/relay-panel
