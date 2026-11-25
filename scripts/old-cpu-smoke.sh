#!/usr/bin/env bash
# Run a built x86_64 binary on an emulated CPU without AVX/AVX2/BMI2 and make
# sure it still starts. Catches C code (bundled SQLite, ring) compiled for a
# newer x86-64 level than our baseline, which crashes older hosts with SIGILL
# ("invalid opcode") instead of failing any test on the build machine.
#
#   bash scripts/old-cpu-smoke.sh panel target/release/relay-panel
#   bash scripts/old-cpu-smoke.sh node  target/x86_64-unknown-linux-musl/release/relay-node
#
# The binary runs under qemu-x86_64-static inside a debian:bookworm-slim
# container (the panel image's base, with Debian's qemu-user-static added;
# built here on first use), so it loads that image's libraries. Running it on
# the host instead would load the host's glibc, which may itself need a newer
# CPU (Ubuntu 26.04's refuses to start below x86-64-v3) and would fail every
# binary. The host only needs docker (and gdb, to name a crash site).
#
# OLD_CPU picks the emulated model; the default, Nehalem, has SSE4.2 but no
# AVX at all.
set -euo pipefail

KIND=${1:?usage: old-cpu-smoke.sh panel|node <binary>}
BIN=$(realpath "${2:?usage: old-cpu-smoke.sh panel|node <binary>}")
CPU=${OLD_CPU:-Nehalem}
IMAGE=relaypanel-old-cpu:bookworm
command -v docker >/dev/null || { echo "old-cpu-smoke: docker not found" >&2; exit 2; }
if ! docker image inspect "$IMAGE" >/dev/null 2>&1; then
    printf '%s\n' \
        'FROM debian:bookworm-slim' \
        'RUN apt-get update && apt-get install -y --no-install-recommends qemu-user-static && rm -rf /var/lib/apt/lists/*' \
        | docker build -q -t "$IMAGE" - >/dev/null
fi

WORK=$(mktemp -d)
chmod 777 "$WORK"
LOG="$WORK/out.log"
NAME="old-cpu-smoke-$$"
cleanup() {
    docker rm -f "$NAME" >/dev/null 2>&1 || true
    rm -rf "$WORK" 2>/dev/null || sudo rm -rf "$WORK" 2>/dev/null || true
}
trap cleanup EXIT

# run_in_box <env...> -- <command...>: the command runs in the container with
# the binary at /under-test; the work dir is /work.
run_in_box() {
    local envs=()
    while [ "$1" != "--" ]; do envs+=(-e "$1"); shift; done
    shift
    docker run --rm --name "$NAME" --network host --ulimit core=-1 \
        -v "$BIN:/under-test:ro" -v "$WORK:/work" -w /work \
        "${envs[@]}" "$IMAGE" "$@"
}

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
    run_in_box \
        "DATABASE_URL=sqlite:/work/data.db?mode=rwc" \
        "LISTEN=127.0.0.1:$PORT" \
        "PUBLIC_DIR=/work" \
        "JWT_SECRET=old-cpu-smoke-0123456789abcdef0123456789abcdef" \
        "GEOIP_ENABLED=false" \
        -- qemu-x86_64-static -cpu "$CPU" /under-test >"$LOG" 2>&1 &
    PID=$!
    for _ in $(seq 1 300); do
        if ! kill -0 "$PID" 2>/dev/null; then
            set +e; wait "$PID"; rc=$?; set -e
            [ "$rc" -eq 132 ] && fail "illegal instruction (exit 132): the binary needs a newer CPU than $CPU"
            fail "exited early with code $rc"
        fi
        # A login attempt with a made-up user queries the users table.
        code=$(curl -s -o /dev/null -w '%{http_code}' -X POST "http://127.0.0.1:$PORT/api/v1/auth/login" \
            -H 'Content-Type: application/json' --data '{"username":"old-cpu-smoke","password":"x"}' || true)
        if [ -n "$code" ] && [ "$code" != 000 ]; then
            echo "old-cpu-smoke: panel answered HTTP $code on emulated $CPU (bookworm)"
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
    run_in_box "PANEL_URL=https://example.com" "NODE_TOKEN=old-cpu-smoke-token" \
        -- timeout 45 qemu-x86_64-static -cpu "$CPU" /under-test >"$LOG" 2>&1
    rc=$?
    set -e
    case "$rc" in
        124) echo "old-cpu-smoke: node ran 45s on emulated $CPU (bookworm)" ;;
        132) fail "illegal instruction (exit 132): the binary needs a newer CPU than $CPU" ;;
        *)   fail "exited early with code $rc" ;;
    esac
    ;;
*)
    echo "usage: old-cpu-smoke.sh panel|node <binary>" >&2
    exit 2
    ;;
esac
