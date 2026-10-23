#!/usr/bin/env bash
# Exercise scripts/mirror-lib.sh (the download-acceleration helpers shared by
# deploy.sh and the panel updater) without Docker, GitHub or a network.
#
# `docker` and `curl` are replaced by stubs on PATH. Which registries "answer"
# docker (manifest inspect / pull) is set per scenario in $S/reachable (one
# registry host per line); which ones answer the registry HTTP API (the
# digest lookup) in $S/digest-hosts (default: the same list). Every docker call
# is logged to $S/calls and every curl call to $S/curl-calls, so the order of
# probes, pulls and tags can be checked.
#
# The point of most scenarios is the trust model: public mirrors are only ever
# pulled BY DIGEST, with a digest a trusted registry reported.
#
#   bash scripts/test-mirror-lib.sh
set -uo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
FAILS=0

check() { # DESCRIPTION CONDITION...
    local what="$1"; shift
    if "$@"; then echo "  ok   $what"; else echo "  FAIL $what"; FAILS=$((FAILS + 1)); fi
}

T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT
S="$T/state"
mkdir -p "$S" "$T/bin"

DIGEST="sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
OTHER_DIGEST="sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"

cat > "$T/bin/docker" <<EOF
#!/usr/bin/env bash
echo "docker \$*" >> "$S/calls"
host_of() { echo "\${1%%/*}"; }
reachable() { # IMAGE_REF -> its registry host is listed in $S/reachable
    local h; h="\$(host_of "\$1")"
    case "\$1" in */*) case "\$h" in *.*|*:*) ;; *) h=docker.io ;; esac ;; *) h=docker.io ;; esac
    grep -qx "\$h" "$S/reachable" 2>/dev/null
}
case "\$1 \$2" in
"manifest --help") [ -f "$S/no-manifest" ] && exit 1; exit 0 ;;
"manifest inspect") reachable "\$3"; exit \$? ;;
"pull "*)
    if grep -qxF "\$2" "$S/pull-fails" 2>/dev/null; then echo "pull failed: \$2" >&2; exit 1; fi
    exit 0 ;;
"image inspect")
    if [ -f "$S/inspect-out" ]; then cat "$S/inspect-out"; else echo '["x@$DIGEST"]'; fi
    exit 0 ;;
"compose -f")
    case "\$*" in *" config --images") cat "$S/images" 2>/dev/null; exit 0 ;; esac
    exit 0 ;;
esac
exit 0
EOF

# A registry HTTP API: the first manifest request is answered 401 with a Bearer
# challenge, the token endpoint hands out a token, and the manifest request
# carrying it gets the digest. Only hosts listed in $S/digest-hosts (default
# $S/reachable) answer; registry-1.docker.io stands for docker.io.
cat > "$T/bin/curl" <<EOF
#!/usr/bin/env bash
echo "curl \$*" >> "$S/curl-calls"
url=""; auth=0
for a in "\$@"; do
    case "\$a" in
    "Authorization: Bearer "*) auth=1 ;;
    https://*) url="\$a" ;;
    esac
done
host="\${url#https://}"; host="\${host%%/*}"
[ "\$host" != registry-1.docker.io ] || host=docker.io
list="$S/digest-hosts"; [ -f "\$list" ] || list="$S/reachable"
grep -qx "\$host" "\$list" 2>/dev/null || exit 7
case "\$url" in
*/token*) echo '{"token":"tok"}' ;;
*/manifests/*)
    if [ "\$auth" = 0 ]; then
        printf 'HTTP/2 401\r\nwww-authenticate: Bearer realm="https://%s/token",service="%s"\r\n\r\n' "\$host" "\$host"
    elif [ -f "$S/bad-digest" ]; then
        printf 'HTTP/2 200\r\ndocker-content-digest: sha256:nothex\r\n\r\n'
    else
        printf 'HTTP/2 200\r\ndocker-content-digest: $DIGEST\r\n\r\n'
    fi ;;
esac
exit 0
EOF
chmod +x "$T/bin/docker" "$T/bin/curl"

# new_case: clean state; the library is sourced fresh by each run_lib.
new_case() { rm -f "$S"/*; : > "$S/calls"; : > "$S/curl-calls"; }
calls() { cat "$S/calls"; }
has_call() { grep -qxF "$1" "$S/calls"; }
no_call() { ! grep -qF "$1" "$S/calls"; }
nth_call_is() { [ "$(sed -n "${1}p" "$S/calls")" = "$2" ]; }
said() { grep -qF "$1" "$S/say"; }

# run_lib 'shell code': runs in a subshell with the library loaded.
run_lib() {
    ( PATH="$T/bin:$PATH"
      rp_say() { echo "$*" >> "$S/say"; }
      # shellcheck source=mirror-lib.sh
      . "$HERE/mirror-lib.sh"
      eval "$1" )
}

PANEL=ghcr.io/moeshinx/relay-panel-panel:1.2.15
NODE=ghcr.io/moeshinx/relay-panel-node:1.2.9
NODE_NAMED=moeshinx/relay-panel-node
PANEL_NAMED=moeshinx/relay-panel-panel

echo "image names"
new_case
out="$(run_lib 'rp_split_image '"$PANEL"'; rp_image_routes '"$PANEL")"
check "ghcr image: ghcr.io, down.relaypanel.dev, nju" test "$out" = "0 ghcr.io $PANEL
1 down.relaypanel.dev down.relaypanel.dev/$PANEL_NAMED:1.2.15
2 ghcr.nju.edu.cn ghcr.nju.edu.cn/$PANEL_NAMED:1.2.15"
out="$(run_lib 'rp_split_image caddy:2-alpine; rp_image_routes caddy:2-alpine')"
check "official Docker Hub image gets library/" test "$out" = "0 docker.io caddy:2-alpine
1 down.relaypanel.dev down.relaypanel.dev/library/caddy:2-alpine
2 docker.m.daocloud.io docker.m.daocloud.io/library/caddy:2-alpine
3 docker.1ms.run docker.1ms.run/library/caddy:2-alpine"
out="$(run_lib 'rp_split_image someone/tool:1; rp_image_routes someone/tool:1' | sed -n 2p)"
check "user image on Docker Hub keeps its namespace" test "$out" = "1 down.relaypanel.dev down.relaypanel.dev/someone/tool:1"
out="$(run_lib 'rp_split_image docker.io/library/postgres:16-alpine; rp_image_routes docker.io/library/postgres:16-alpine' | sed -n '1p;3p')"
check "docker.io/ spelling is the same Hub family" test "$out" = "0 docker.io docker.io/library/postgres:16-alpine
2 docker.m.daocloud.io docker.m.daocloud.io/library/postgres:16-alpine"
out="$(run_lib 'rp_split_image quay.io/x/y:1; rp_image_routes quay.io/x/y:1')"
check "other registries have no mirrors" test "$out" = "0 quay.io quay.io/x/y:1"
out="$(run_lib 'rp_split_image localhost:5000/app:1; rp_image_routes localhost:5000/app:1')"
check "a host:port registry is not Docker Hub" test "$out" = "0 localhost:5000 localhost:5000/app:1"
out="$(run_lib 'rp_split_image '"$PANEL"'; echo "$RP_IMG_REPO $RP_IMG_TAG"; rp_split_image caddy; echo "$RP_IMG_REPO $RP_IMG_TAG"')"
check "repository and tag are split (latest when absent)" test "$out" = "$PANEL_NAMED 1.2.15
library/caddy latest"

echo "route order"
out="$(run_lib 'rp_order 4 -1' | tr '\n' ' ')"
check "no preference: list order" test "$out" = "0 1 2 3 "
out="$(run_lib 'rp_order 4 2' | tr '\n' ' ')"
check "the route that worked goes first" test "$out" = "2 0 1 3 "
out="$(run_lib 'printf "%s|" "${RP_GITHUB_PREFIXES[@]}"; echo "$RP_TRUSTED_COUNT"')"
check "GitHub: direct, down.relaypanel.dev (trusted), gh-proxy, ghfast (public)" test "$out" = "|https://down.relaypanel.dev/|https://gh-proxy.com/|https://ghfast.top/|2"

echo "ghcr.io reachable"
new_case; echo ghcr.io > "$S/reachable"
run_lib "rp_pull_image $PANEL"; rc=$?
check "pulled" test $rc -eq 0
check "one probe, one pull, nothing else" test "$(calls | grep -vc 'manifest --help')" -eq 2
check "pulled under its own name, by tag" has_call "docker pull $PANEL"
check "no tag needed" no_call "docker tag"
check "no digest lookup was needed" test ! -s "$S/curl-calls"

echo "ghcr.io blocked, down.relaypanel.dev (trusted) works: pulled by tag"
new_case; echo down.relaypanel.dev > "$S/reachable"
run_lib "rp_pull_image $PANEL"; rc=$?
check "pulled" test $rc -eq 0
check "ghcr.io probed first" nth_call_is 2 "docker manifest inspect $PANEL"
check "then down.relaypanel.dev" nth_call_is 3 "docker manifest inspect down.relaypanel.dev/$PANEL_NAMED:1.2.15"
check "pulled by tag (quietly, so docker does not print the relay address)" has_call "docker pull -q down.relaypanel.dev/$PANEL_NAMED:1.2.15"
check "messages call it the mirror, never by address" bash -c "grep -q \"via mirror\" \"$S/say\" && ! grep -q relaypanel \"$S/say\""
check "tagged back to the name compose uses" has_call "docker tag down.relaypanel.dev/$PANEL_NAMED:1.2.15 $PANEL"
check "the mirror's name is dropped" has_call "docker rmi down.relaypanel.dev/$PANEL_NAMED:1.2.15"
check "ghcr.io was never pulled" no_call "docker pull $PANEL"
check "no digest lookup for a trusted route" test ! -s "$S/curl-calls"

echo "only a public mirror reaches docker: pulled by digest from a trusted registry"
new_case; echo ghcr.nju.edu.cn > "$S/reachable"; echo ghcr.io > "$S/digest-hosts"
run_lib "rp_pull_image $NODE"; rc=$?
check "pulled" test $rc -eq 0
check "pulled by digest" has_call "docker pull ghcr.nju.edu.cn/$NODE_NAMED@$DIGEST"
check "never by tag" no_call "docker pull ghcr.nju.edu.cn/$NODE_NAMED:1.2.9"
check "the probe asked for the digest reference too" has_call "docker manifest inspect ghcr.nju.edu.cn/$NODE_NAMED@$DIGEST"
check "tagged back" has_call "docker tag ghcr.nju.edu.cn/$NODE_NAMED@$DIGEST $NODE"
check "the mirror's name is dropped" has_call "docker rmi ghcr.nju.edu.cn/$NODE_NAMED@$DIGEST"
check "the digest came from ghcr.io" grep -q 'https://ghcr.io/v2/moeshinx/relay-panel-node/manifests/1.2.9' "$S/curl-calls"
check "the registry token scope is URL-encoded" grep -q 'scope=repository%3Amoeshinx%2Frelay-panel-node%3Apull' "$S/curl-calls"
check "the public mirror was never asked for a digest" bash -c "! grep -q 'nju' '$S/curl-calls'"

echo "the digest comes from down.relaypanel.dev when ghcr.io does not answer"
new_case; echo ghcr.nju.edu.cn > "$S/reachable"; echo down.relaypanel.dev > "$S/digest-hosts"
run_lib "rp_pull_image $PANEL"; rc=$?
check "pulled by digest" test $rc -eq 0 -a "$(calls | grep -c "docker pull ghcr.nju.edu.cn/$PANEL_NAMED@$DIGEST")" -eq 1
check "ghcr.io was tried for the digest first" test "$(grep -n 'ghcr.io/v2' "$S/curl-calls" | head -1 | cut -d: -f1)" -lt "$(grep -n 'down.relaypanel.dev/v2' "$S/curl-calls" | head -1 | cut -d: -f1)"

echo "no trusted registry gives a digest: public mirrors are not used"
new_case; echo ghcr.nju.edu.cn > "$S/reachable"; : > "$S/digest-hosts"
run_lib "rp_pull_image $PANEL"; rc=$?
check "fails" test $rc -eq 1
check "nothing was pulled" no_call "docker pull"
check "the public mirror was not even probed" no_call "docker manifest inspect ghcr.nju.edu.cn"
check "says why" said "no trusted registry gave the digest"
new_case; echo ghcr.nju.edu.cn > "$S/reachable"; echo ghcr.io > "$S/digest-hosts"; touch "$S/bad-digest"
run_lib "rp_pull_image $PANEL"; rc=$?
check "a malformed digest is no digest" test $rc -eq 1 -a "$(calls | grep -c 'docker pull')" -eq 0

echo "a public pull whose image does not carry the digest is rejected"
new_case; echo ghcr.nju.edu.cn > "$S/reachable"; echo ghcr.io > "$S/digest-hosts"
echo "[\"x@$OTHER_DIGEST\"]" > "$S/inspect-out"
run_lib "rp_pull_image $PANEL"; rc=$?
check "fails" test $rc -eq 1
check "not tagged as the official name" no_call "docker tag"
check "the mirror's image is removed" has_call "docker rmi ghcr.nju.edu.cn/$PANEL_NAMED@$DIGEST"

echo "a trusted pull that fails: the next route is tried, public ones by digest"
new_case; printf 'ghcr.io\nghcr.nju.edu.cn\n' > "$S/reachable"
echo "$PANEL" > "$S/pull-fails"
run_lib "rp_pull_image $PANEL"; rc=$?
check "succeeds via the public registry" test $rc -eq 0
check "by digest" has_call "docker pull ghcr.nju.edu.cn/$PANEL_NAMED@$DIGEST"
check "tagged back" has_call "docker tag ghcr.nju.edu.cn/$PANEL_NAMED@$DIGEST $PANEL"
check "the unreachable mirror between was skipped" no_call "docker pull down.relaypanel.dev"

echo "nothing reachable"
new_case
run_lib "rp_pull_image $PANEL"; rc=$?
check "fails" test $rc -eq 1
check "nothing was pulled" no_call "docker pull"

echo "Docker Hub image"
new_case; echo down.relaypanel.dev > "$S/reachable"
run_lib 'rp_pull_image caddy:2-alpine'; rc=$?
check "trusted mirror: pulled by tag with library/" test $rc -eq 0 -a "$(calls | grep -c 'docker pull -q down.relaypanel.dev/library/caddy:2-alpine$')" -eq 1
check "tagged back to caddy:2-alpine" has_call "docker tag down.relaypanel.dev/library/caddy:2-alpine caddy:2-alpine"
new_case; echo docker.m.daocloud.io > "$S/reachable"; printf 'docker.io\n' > "$S/digest-hosts"
run_lib 'rp_pull_image caddy:2-alpine'; rc=$?
check "public mirror: pulled" test $rc -eq 0
check "by digest, with library/" has_call "docker pull docker.m.daocloud.io/library/caddy@$DIGEST"
check "never by tag" no_call "docker pull docker.m.daocloud.io/library/caddy:2-alpine"
check "tagged back to caddy:2-alpine" has_call "docker tag docker.m.daocloud.io/library/caddy@$DIGEST caddy:2-alpine"
check "the digest came from Docker Hub's registry" grep -q 'https://registry-1.docker.io/v2/library/caddy/manifests/2-alpine' "$S/curl-calls"
new_case; echo docker.io > "$S/reachable"
run_lib 'rp_pull_image postgres:16-alpine'; rc=$?
check "direct Docker Hub: plain pull, no tag" test $rc -eq 0 -a "$(calls | grep -c 'docker tag')" -eq 0

echo "the route that worked is remembered within one run"
new_case; echo down.relaypanel.dev > "$S/reachable"
run_lib "rp_pull_image $PANEL; : > '$S/calls'; rp_pull_image $NODE"; rc=$?
check "second image succeeds" test $rc -eq 0
check "starts at the mirror that worked, not at ghcr.io" nth_call_is 1 "docker manifest inspect down.relaypanel.dev/$NODE_NAMED:1.2.9"
check "and does not probe ghcr.io at all" no_call "docker manifest inspect $NODE"
new_case; echo down.relaypanel.dev > "$S/reachable"
run_lib "rp_pull_image $PANEL"
new_case; echo ghcr.io > "$S/reachable"
run_lib "rp_pull_image $NODE"; rc=$?
check "a new run starts from the direct registry again" nth_call_is 2 "docker manifest inspect $NODE"

echo "a remembered public route does not skip the digest rule"
new_case; printf 'ghcr.nju.edu.cn\n' > "$S/reachable"; echo ghcr.io > "$S/digest-hosts"
run_lib "rp_pull_image $PANEL; rp_pull_image $NODE"; rc=$?
check "both pulled" test $rc -eq 0
check "both by digest" test "$(calls | grep -c 'docker pull .*@sha256:')" -eq 2
check "never by tag" bash -c "! grep -E 'docker pull .*:[0-9]' '$S/calls'"

echo "Docker without 'docker manifest'"
new_case; touch "$S/no-manifest"; echo ghcr.io > "$S/reachable"
run_lib "rp_pull_image $PANEL"; rc=$?
check "pulls without probing" test $rc -eq 0 -a "$(calls | grep -c 'manifest inspect')" -eq 0
check "pulled directly" has_call "docker pull $PANEL"

echo "compose images"
new_case; echo ghcr.io > "$S/reachable"; echo docker.m.daocloud.io >> "$S/reachable"
printf 'ghcr.io\ndocker.io\n' > "$S/digest-hosts"
printf '%s\ncaddy:2-alpine\n' "$PANEL" > "$S/images"
run_lib 'rp_pull_compose_images -f docker-compose.release.yaml --profile caddy'; rc=$?
check "all pulled" test $rc -eq 0
check "the image list honours the profiles" has_call "docker compose -f docker-compose.release.yaml --profile caddy config --images"
check "panel image pulled" has_call "docker pull $PANEL"
check "caddy image pulled through the public mirror by digest" has_call "docker pull docker.m.daocloud.io/library/caddy@$DIGEST"
check "caddy tagged back" has_call "docker tag docker.m.daocloud.io/library/caddy@$DIGEST caddy:2-alpine"
new_case
printf '%s\n' "$PANEL" > "$S/images"
run_lib 'rp_pull_compose_images -f docker-compose.release.yaml'; rc=$?
check "an image no registry has: failure" test $rc -eq 1
new_case
run_lib 'rp_pull_compose_images -f docker-compose.release.yaml'; rc=$?
check "no image list: 2, so the caller falls back to compose pull" test $rc -eq 2

echo
if [ "$FAILS" -eq 0 ]; then echo "ALL PASSED"; else echo "$FAILS FAILED"; exit 1; fi
