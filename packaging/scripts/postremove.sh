#!/bin/sh
set -eu

# Keep /var/lib/relay-panel and the relaypanel account so that removing a
# package never discards the operator's database. A later installation reuses
# them. Package managers remove the unit file themselves.
if command -v systemctl >/dev/null 2>&1; then
    systemctl daemon-reload || true
fi
