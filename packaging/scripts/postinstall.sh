#!/bin/sh
set -eu

if command -v systemctl >/dev/null 2>&1; then
    systemctl daemon-reload || true
fi

echo "RelayPanel installed. Edit /etc/relay-panel/relay-panel.env, then run:"
echo "  systemctl enable --now relaypanel"
