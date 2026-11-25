#!/usr/bin/env bash
# Run a built x86_64 binary on an emulated CPU without AVX/AVX2/BMI2 and make
# sure it still starts. Catches C code (bundled SQLite, ring) compiled for a
# newer x86-64 level than our baseline, which crashes older hosts with SIGILL
# ("invalid opcode") instead of failing any test on the build machine.
#
#   bash scripts/old-cpu-smoke.sh panel target/release/relay-panel
#   bash scripts/old-cpu-smoke.sh node  target/x86_64-unknown-linux-musl/release/relay-node
#
# Needs qemu-x86_64 (Debian/Ubuntu package: qemu-user). OLD_CPU picks the
# emulated model; the default, Nehalem, has SSE4.2 but no AVX at all.
#
# qemu-user loads a dynamically linked binary's libraries from the host, and a
# host's own glibc may itself need a newer CPU (Ubuntu 26.04's refuses to run
# below x86-64-v3), which would fail every binary. OLD_CPU_SYSROOT points at
# the root filesystem the binary really runs on — for the panel, the
# debian:bookworm-slim image it ships in — and qemu takes libraries from there.
set -euo pipefail

KIND=${1:?usage: old-cpu-smoke.sh panel|node <binary>}
BIN=$(realpath "${2:?usage: old-cpu-smoke.sh panel|node <binary>}")
CPU=${OLD_CPU:-Nehalem}
QEMU=${QEMU:-qemu-x86_64}
command -v "$QEMU" >/dev/null || { echo "old-cpu-smoke: $QEMU not found (install qemu-user)" >&2; exit 2; }
QEMU_ARGS=(-cpu "$CPU")
if [ -n "${OLD_CPU_SYSROOT:-}" ]; then
    QEMU_ARGS+=(-L "$OLD_CPU_SYSROOT")
fi

WORK=$(mktemp -d)
LOG="$WORK/out.log"
PID=
cleanup() {
    if [ -n "$PID" ]; then kill "$PID" 2>/dev/null || true; fi
    rm -rf "$WORK"
}
trap cleanup EXIT

fail() {
    echo "::error::old-cpu-smoke ($KIND on $CPU): $1" >&2
    echo "--- last output ---" >&2
    tail -n 40 "$LOG" >&2 || true
    # qemu-user writes the guest's core file into the working directory; with
    # gdb at hand, name the function and instruction that trapped.
    core=$(find "$WORK" -maxdepth 1 -name 'qemu_*.core' 2>/dev/null | head -n 1)
    if [ -n "$core" ] && command -v gdb >/dev/null; then
        echo "--- faulting instruction ---" >&2
        gdb -q -batch -ex 'info symbol $pc' -ex 'x/3i $pc' -ex 'bt 8' "$BIN" "$core" >&2 2>&1 || true
    fi
    exit 1
}

case "$KIND" in
panel)
    PORT=${OLD_CPU_PORT:-18997}
    # Opening the SQLite database and running every migration happens before
    # the listener comes up, so an HTTP answer means SQLite ran on this CPU.
    (
        cd "$WORK"
        ulimit -c unlimited 2>/dev/null || true
        DATABASE_URL="sqlite:$WORK/data.db?mode=rwc" \
        LISTEN="127.0.0.1:$PORT" \
        PUBLIC_DIR="$WORK" \
        JWT_SECRET="old-cpu-smoke-0123456789abcdef0123456789abcdef" \
        GEOIP_ENABLED=false \
        exec "$QEMU" "${QEMU_ARGS[@]}" "$BIN"
    ) >"$LOG" 2>&1 &
    PID=$!
    for _ in $(seq 1 300); do
        if ! kill -0 "$PID" 2>/dev/null; then
            set +e; wait "$PID"; rc=$?; set -e
            PID=
            [ "$rc" -eq 132 ] && fail "illegal instruction (exit 132): the binary needs a newer CPU than $CPU"
            fail "exited early with code $rc"
        fi
        # A login attempt with a made-up user queries the users table.
        code=$(curl -s -o /dev/null -w '%{http_code}' -X POST "http://127.0.0.1:$PORT/api/v1/auth/login" \
            -H 'Content-Type: application/json' --data '{"username":"old-cpu-smoke","password":"x"}' || true)
        if [ -n "$code" ] && [ "$code" != 000 ]; then
            echo "old-cpu-smoke: panel answered HTTP $code on emulated $CPU"
            exit 0
        fi
        sleep 1
    done
    fail "no HTTP answer within 300s"
    ;;
node)
    # The node reaches the panel over HTTPS (rustls + ring) right away; an
    # unreachable or wrong panel only makes it retry, so it must still be
    # running when the timeout ends it.
    set +e
    (
        cd "$WORK"
        ulimit -c unlimited 2>/dev/null || true
        PANEL_URL="https://example.com" NODE_TOKEN="old-cpu-smoke-token" \
        exec timeout 45 "$QEMU" "${QEMU_ARGS[@]}" "$BIN"
    ) >"$LOG" 2>&1
    rc=$?
    set -e
    case "$rc" in
        124) echo "old-cpu-smoke: node ran 45s on emulated $CPU" ;;
        132) fail "illegal instruction (exit 132): the binary needs a newer CPU than $CPU" ;;
        *)   fail "exited early with code $rc" ;;
    esac
    ;;
*)
    echo "usage: old-cpu-smoke.sh panel|node <binary>" >&2
    exit 2
    ;;
esac
