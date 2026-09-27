#!/usr/bin/env bash
# Exercise scripts/panel-updater.sh without root, systemd, Docker or a network.
#
# git, docker, curl and flock are replaced by stubs on PATH, and deploy.sh by a
# stub in a throw-away copy of the repository. Each scenario checks the status
# file the panel will read: its state, that it is valid JSON, and that the
# request file was consumed (otherwise the systemd path unit re-triggers).
#
#   bash scripts/test-panel-updater.sh
set -uo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
PY="$(command -v python3 || command -v python)"
FAILS=0

check() { # DESCRIPTION CONDITION...
    local what="$1"; shift
    if "$@"; then echo "  ok   $what"; else echo "  FAIL $what"; FAILS=$((FAILS + 1)); fi
}

state_is() { grep -q "\"state\":\"$1\"" "$REPO/run/update-status.json"; }
message_has() { grep -q "$1" "$REPO/run/update-status.json"; }
valid_json() { "$PY" -c 'import json,sys; json.load(open(sys.argv[1], encoding="utf-8"))' "$REPO/run/update-status.json"; }
request_gone() { [ ! -e "$REPO/run/update-request" ]; }

# setup PANEL_VERSION_BEFORE TAG_AFTER_PULL
setup() {
    T="$(mktemp -d)"
    REPO="$T/repo"; STATE="$T/state"; STUBS="$T/bin"
    mkdir -p "$REPO/scripts" "$REPO/run" "$STATE/data" "$STUBS"
    cp "$HERE/panel-updater.sh" "$REPO/scripts/"
    echo "$1" > "$STATE/version"
    compose() { printf 'services:\n  panel:\n    image: ghcr.io/moeshinx/relay-panel-panel:${RELAYPANEL_PANEL_TAG:-%s}\n' "$1"; }
    compose "$1" > "$REPO/docker-compose.release.yaml"
    compose "$2" > "$STATE/compose-after-pull"
    : > "$REPO/.env"
    echo 'sqlite bytes' > "$STATE/data/data.db"
    echo '{"target":"ignored"}' > "$REPO/run/update-request"

    cat > "$REPO/deploy.sh" <<EOF
#!/usr/bin/env bash
[ -f "$STATE/deploy-fails" ] && { echo "deploy exploded"; exit 1; }
cp "$STATE/next-version" "$STATE/version" 2>/dev/null || true
echo "deployed"
EOF
    cat > "$STUBS/curl" <<EOF
#!/usr/bin/env bash
printf '{"status":"ok","version":"%s"}' "\$(cat "$STATE/version")"
EOF
    cat > "$STUBS/git" <<EOF
#!/usr/bin/env bash
[ -f "$STATE/git-fails" ] && { echo "fatal: local changes would be overwritten"; exit 1; }
cp "$STATE/compose-after-pull" "$REPO/docker-compose.release.yaml"
EOF
    cat > "$STUBS/docker" <<EOF
#!/usr/bin/env bash
echo "docker \$*" >> "$STATE/docker-calls"
case "\$*" in
  *"ps -q panel"*) echo cid123 ;;
  inspect*) echo "$STATE/data" ;;
esac
exit 0
EOF
    printf '#!/usr/bin/env bash\nexit 0\n' > "$STUBS/flock"
    chmod +x "$STUBS"/* "$REPO/deploy.sh"
}

run_updater() {
    PATH="$STUBS:$PATH" \
    RELAYPANEL_UPDATER_LOG="$T/updater.log" \
    RELAYPANEL_UPDATER_LOCK="$T/updater.lock" \
        bash "$REPO/scripts/panel-updater.sh" >/dev/null 2>&1
}

echo "pinned version"
setup 1.2.10 1.2.11
echo 'RELAYPANEL_PANEL_TAG=1.2.10' > "$REPO/.env"
run_updater
check "reports pinned" state_is pinned
check "names the variable" message_has RELAYPANEL_PANEL_TAG
check "status is valid JSON" valid_json
check "request consumed" request_gone

echo "git pull fails"
setup 1.2.10 1.2.11
touch "$STATE/git-fails"
run_updater
check "reports failed" state_is failed
check "says git pull" message_has "git pull failed"
check "log tail carries git's error" message_has "local changes would be overwritten"
check "status is valid JSON" valid_json
check "nothing was stopped" bash -c "! grep -q stop '$STATE/docker-calls' 2>/dev/null"

echo "already up to date"
setup 1.2.10 1.2.10
run_updater
check "reports up_to_date" state_is up_to_date
check "panel was not restarted" bash -c "! grep -q stop '$STATE/docker-calls' 2>/dev/null"
check "request consumed" request_gone

echo "successful update with SQLite backup"
setup 1.2.10 1.2.11
echo 1.2.11 > "$STATE/next-version"
run_updater
check "reports succeeded" state_is succeeded
check "records the new version" grep -q '"to_version":"1.2.11"' "$REPO/run/update-status.json"
check "panel stopped before the copy" grep -q "stop panel" "$STATE/docker-calls"
check "database backed up" bash -c "ls '$REPO'/backups/data-*-v1.2.10.db >/dev/null 2>&1"
check "message mentions the backup" message_has "backed up"
check "status is valid JSON" valid_json
check "request consumed" request_gone

echo "deploy.sh fails"
setup 1.2.10 1.2.11
touch "$STATE/deploy-fails"
run_updater
check "reports failed" state_is failed
check "tries to bring the panel back" grep -q "up -d panel" "$STATE/docker-calls"
check "status is valid JSON" valid_json

echo "a request planted as a directory"
setup 1.2.10 1.2.10
rm -f "$REPO/run/update-request"
mkdir -p "$REPO/run/update-request/nested"
run_updater
check "the directory is removed (no re-trigger loop)" request_gone
check "the run still completes" state_is up_to_date

echo "old backups are pruned to five"
setup 1.2.10 1.2.11
echo 1.2.11 > "$STATE/next-version"
mkdir -p "$REPO/backups"
for i in 1 2 3 4 5 6 7; do
    touch -d "2020-01-0$i" "$REPO/backups/data-2020010$i-000000-v1.0.$i.db" 2>/dev/null \
        || touch "$REPO/backups/data-2020010$i-000000-v1.0.$i.db"
done
run_updater
check "five backups left" bash -c "[ \$(ls '$REPO'/backups/data-*.db | wc -l) -eq 5 ]"

echo
if [ "$FAILS" -eq 0 ]; then echo "ALL PASSED"; else echo "$FAILS FAILED"; exit 1; fi
