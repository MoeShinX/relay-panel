#!/usr/bin/env bash
# shellcheck shell=bash
#
# Download acceleration for GitHub and container registries — sourced by
# deploy.sh and scripts/panel-updater.sh. Do not run it directly.
#
# install.sh cannot source this file (it runs before the repository exists), so
# it carries its own copy of the prefix list and the fetch loop. The panel's
# version check (crates/panel/src/api/system.rs) has a third copy of the trusted
# part of the list. Keep all three in sync.
#
# TRUST MODEL. What is fetched here ends up running as root (the repository's
# deploy.sh, container images), so a route is only used for bulk data when the
# data can be checked against something a trusted source said:
#
#   TRUSTED  direct GitHub / direct registries, and a trusted relay (the first
#            two entries of every list below; the second is the relay).
#   PUBLIC   gh-proxy.com, ghfast.top, ghcr.nju.edu.cn, docker.m.daocloud.io,
#            docker.1ms.run (the rest of the lists).
#
#   Git:    the commit id of main is read from a trusted source only. Every
#           route, public ones included, is then asked for exactly that commit
#           (`git fetch <url> <id>`) and the result must be that id. Git ids are
#           content hashes, so a match proves the content. No trusted answer =>
#           nothing is fetched at all.
#   Images: the manifest digest is read from a trusted registry only. Trusted
#           routes are pulled by tag; public routes ONLY as <mirror>/<repo>@sha256:<digest>,
#           which Docker verifies against the digest itself. No trusted digest
#           => public routes are not used for that image.
#
# Everything is tried in a fixed order, direct first. Within ONE run the route
# that worked is tried first for the rest of that run; the next run starts
# from direct again.
#
#   GitHub (git): a prefix in front of the full GitHub URL.
#   Registries: the registry host in the image name is replaced.
#
# The caller defines:
#   rp_say MESSAGE...   print one progress line (this file never exits)
# and may set:
#   RP_GIT=(git ...)    how to run git (default: git)

RP_REPO_URL="https://github.com/MoeShinX/relay-panel.git"

# Order matters. The empty prefix is a direct connection. The first
# RP_TRUSTED_COUNT entries of each list are TRUSTED, the rest PUBLIC.
RP_TRUSTED_COUNT=2
RP_GITHUB_PREFIXES=(
    ""
    "https://down.relaypanel.dev/"
    "https://gh-proxy.com/"
    "https://ghfast.top/"
)
# Images under ghcr.io (this project's images).
RP_GHCR_REGISTRIES=(ghcr.io down.relaypanel.dev ghcr.nju.edu.cn)
# Images on Docker Hub (caddy, postgres, ...).
RP_HUB_REGISTRIES=(docker.io down.relaypanel.dev docker.m.daocloud.io docker.1ms.run)

# Seconds. Connect timeout and the total for one small request / manifest probe.
RP_CONNECT_TIMEOUT=5
RP_PROBE_TIMEOUT=10

# Index of the route that worked in this run, or -1 (none yet).
RP_PREFIX_OK=-1
RP_GHCR_OK=-1
RP_HUB_OK=-1

declare -p RP_GIT >/dev/null 2>&1 || RP_GIT=(git)
declare -F rp_say >/dev/null || rp_say() { echo "$*" >&2; }

# rp_order COUNT PREFERRED -> indexes 0..COUNT-1, PREFERRED (if >= 0) first.
rp_order() {
    local n="$1" pref="$2" i
    if [ "$pref" -ge 0 ]; then echo "$pref"; fi
    for ((i = 0; i < n; i++)); do
        if [ "$i" -ne "$pref" ]; then echo "$i"; fi
    done
}

rp_timeout() { # SECONDS COMMAND... (plain run if there is no timeout(1))
    local secs="$1"
    shift
    if command -v timeout >/dev/null 2>&1; then
        timeout "$secs" "$@"
    else
        "$@"
    fi
}

# ---------------------------------------------------------------------------
# GitHub
# ---------------------------------------------------------------------------

# A name for messages: index 0 is the direct connection, index 1 the trusted
# relay (shown as "mirror", never by its address), the rest by their host.
rp_github_label() { # INDEX PREFIX
    case "$1" in
        0) echo "GitHub (direct)" ;;
        1) echo "mirror" ;;
        *)
            local host="${2#https://}"
            echo "${host%%/*}"
            ;;
    esac
}

# git's own messages quote the URL it was given; do not show the relay's
# address in them.
rp_scrub() { # TEXT
    local text="$1"
    echo "${text//"${RP_GITHUB_PREFIXES[1]}"/(mirror) }"
}

# Can this route serve the repository? 5 s to connect, 10 s in total. Asks for
# the same document `git fetch` starts with.
rp_git_probe() { # FULL_REPO_URL
    local url="${1}/info/refs?service=git-upload-pack"
    if command -v curl >/dev/null 2>&1; then
        curl -fsS -o /dev/null --connect-timeout "$RP_CONNECT_TIMEOUT" \
            --max-time "$RP_PROBE_TIMEOUT" "$url" >/dev/null 2>&1
    else
        rp_timeout "$RP_PROBE_TIMEOUT" env GIT_TERMINAL_PROMPT=0 \
            "${RP_GIT[@]}" ls-remote "$1" HEAD >/dev/null 2>&1
    fi
}

# The commit id of refs/heads/main as told by a TRUSTED source (direct GitHub,
# then the trusted relay). Prints the 40-hex id and returns 0; returns 1 if
# neither answers. Public accelerators are never asked.
rp_git_trusted_main_sha() {
    local idx prefix label full out sha ref
    for ((idx = 0; idx < RP_TRUSTED_COUNT && idx < ${#RP_GITHUB_PREFIXES[@]}; idx++)); do
        prefix="${RP_GITHUB_PREFIXES[$idx]}"
        label="$(rp_github_label "$idx" "$prefix")"
        full="${prefix}${RP_REPO_URL}"
        if ! rp_git_probe "$full"; then
            rp_say "$label: not reachable, trying the next source for the commit id" >&2
            continue
        fi
        out="$(rp_timeout "$RP_PROBE_TIMEOUT" env GIT_TERMINAL_PROMPT=0 \
            "${RP_GIT[@]}" ls-remote "$full" refs/heads/main 2>/dev/null)" || out=""
        while read -r sha ref; do
            if [ "$ref" = "refs/heads/main" ] && [[ "$sha" =~ ^[0-9a-f]{40}$ ]]; then
                echo "$sha"
                return 0
            fi
        done <<<"$out"
        rp_say "$label: did not give the commit id of main, trying the next source" >&2
    done
    return 1
}

# Fast-forward the current repository to GitHub's main, trying each route in
# turn. Only ever a fast-forward: on failure nothing is changed. See the trust
# model at the top: the commit id comes from a trusted source, and every route
# must deliver exactly that commit.
#   0  now at (or already at) the tip of main
#   1  no trusted source gave the commit id, or no route could deliver that
#      commit (the repository is untouched)
#   2  the commit was fetched but cannot be fast-forwarded (local changes that
#      would be overwritten, or history that diverged); git's message was printed
# Also refreshes refs/remotes/origin/main, so `git reset --hard origin/main`
# means what it says. The `origin` remote itself is never rewritten.
rp_git_update() {
    local want idx prefix label full out rc got fetched=0
    if ! want="$(rp_git_trusted_main_sha)"; then
        rp_say "could not get the current commit of main from GitHub or the trusted mirror; public accelerators are not used without a commit id to check against. Nothing was changed."
        return 1
    fi
    rp_say "main is at ${want:0:12}"
    for idx in $(rp_order "${#RP_GITHUB_PREFIXES[@]}" "$RP_PREFIX_OK"); do
        prefix="${RP_GITHUB_PREFIXES[$idx]}"
        label="$(rp_github_label "$idx" "$prefix")"
        full="${prefix}${RP_REPO_URL}"
        if ! rp_git_probe "$full"; then
            rp_say "$label: not reachable, trying the next route"
            continue
        fi
        rp_say "fetching ${want:0:12} via $label"
        out="$(GIT_TERMINAL_PROMPT=0 GIT_HTTP_LOW_SPEED_LIMIT=51200 GIT_HTTP_LOW_SPEED_TIME=15 \
            "${RP_GIT[@]}" fetch --quiet "$full" "$want" 2>&1)" && rc=0 || rc=$?
        [ -z "$out" ] || rp_say "$(rp_scrub "$out")"
        if [ "$rc" -ne 0 ]; then
            rp_say "$label: fetch failed, trying the next route"
            continue
        fi
        got="$("${RP_GIT[@]}" rev-parse --verify --quiet 'FETCH_HEAD^{commit}' 2>/dev/null)" || got=""
        if [ "$got" != "$want" ]; then
            rp_say "$label: delivered ${got:-nothing} instead of ${want:0:12}, rejected; trying the next route"
            continue
        fi
        fetched=1
        RP_PREFIX_OK="$idx"
        break
    done
    [ "$fetched" -eq 1 ] || return 1

    "${RP_GIT[@]}" update-ref refs/remotes/origin/main "$want" >/dev/null 2>&1 || true
    out="$("${RP_GIT[@]}" merge --ff-only --quiet "$want" 2>&1)" && rc=0 || rc=$?
    [ -z "$out" ] || rp_say "$out"
    [ "$rc" -eq 0 ] || return 2
    return 0
}

# ---------------------------------------------------------------------------
# Container images
# ---------------------------------------------------------------------------

# rp_split_image IMAGE -> RP_IMG_REG   registry host (docker.io for Docker Hub)
#                         RP_IMG_PATH  repository[:tag], "library/" added for
#                                      official Docker Hub images
#                         RP_IMG_REPO  repository without the tag
#                         RP_IMG_TAG   the tag (latest if none)
rp_split_image() {
    local img="$1" first last
    first="${img%%/*}"
    if [ "$first" = "$img" ]; then
        RP_IMG_REG="docker.io"
        RP_IMG_PATH="library/$img"
    elif [[ "$first" == *.* || "$first" == *:* || "$first" == "localhost" ]]; then
        RP_IMG_REG="$first"
        RP_IMG_PATH="${img#*/}"
    else
        RP_IMG_REG="docker.io"
        RP_IMG_PATH="$img"
    fi
    case "$RP_IMG_REG" in
        index.docker.io | registry-1.docker.io) RP_IMG_REG="docker.io" ;;
    esac
    if [ "$RP_IMG_REG" = "docker.io" ] && [[ "$RP_IMG_PATH" != */* ]]; then
        RP_IMG_PATH="library/$RP_IMG_PATH"
    fi
    last="${RP_IMG_PATH##*/}"
    if [[ "$last" == *:* ]]; then
        RP_IMG_TAG="${last#*:}"
        RP_IMG_REPO="${RP_IMG_PATH%:"$RP_IMG_TAG"}"
    else
        RP_IMG_TAG="latest"
        RP_IMG_REPO="$RP_IMG_PATH"
    fi
}

# rp_image_routes IMAGE -> "INDEX REGISTRY REF" lines, one per name to try, in
# order (INDEX is the registry's position in its family list, below
# RP_TRUSTED_COUNT = trusted; REF the image name on that registry). The first
# is IMAGE itself exactly as written. Run rp_split_image first so RP_IMG_*
# describe IMAGE. Images from other registries have no mirrors: just themselves.
rp_image_routes() {
    local img="$1" idx reg
    case "$RP_IMG_REG" in
        ghcr.io)
            for idx in $(rp_order "${#RP_GHCR_REGISTRIES[@]}" "$RP_GHCR_OK"); do
                reg="${RP_GHCR_REGISTRIES[$idx]}"
                if [ "$reg" = "ghcr.io" ]; then echo "$idx $reg $img"; else echo "$idx $reg $reg/$RP_IMG_PATH"; fi
            done
            ;;
        docker.io)
            for idx in $(rp_order "${#RP_HUB_REGISTRIES[@]}" "$RP_HUB_OK"); do
                reg="${RP_HUB_REGISTRIES[$idx]}"
                if [ "$reg" = "docker.io" ]; then echo "$idx $reg $img"; else echo "$idx $reg $reg/$RP_IMG_PATH"; fi
            done
            ;;
        *) echo "0 $RP_IMG_REG $img" ;;
    esac
}

# Can this registry serve the image? Asks for its manifest, 10 s at most.
# A Docker too old to have `docker manifest` is not probed (the pull itself then
# decides).
RP_MANIFEST_SUPPORTED=""
rp_manifest_ok() { # IMAGE_REF
    if [ -z "$RP_MANIFEST_SUPPORTED" ]; then
        if docker manifest --help >/dev/null 2>&1; then RP_MANIFEST_SUPPORTED=1; else RP_MANIFEST_SUPPORTED=0; fi
    fi
    [ "$RP_MANIFEST_SUPPORTED" = "1" ] || return 0
    rp_timeout "$RP_PROBE_TIMEOUT" docker manifest inspect "$1" >/dev/null 2>&1
}

RP_MANIFEST_ACCEPT="application/vnd.oci.image.index.v1+json, application/vnd.docker.distribution.manifest.list.v2+json, application/vnd.oci.image.manifest.v1+json, application/vnd.docker.distribution.manifest.v2+json"

# The Docker-Content-Digest header of a HEAD response, if the status was 200.
rp_header_digest() { # HEADERS
    local status digest
    status="$(printf '%s\n' "$1" | head -n1)"
    case "$status" in *" 200"*) ;; *) return 1 ;; esac
    digest="$(printf '%s\n' "$1" | grep -i '^docker-content-digest:' | head -n1 | sed 's/^[^:]*:[[:space:]]*//')"
    [[ "$digest" =~ ^sha256:[0-9a-f]{64}$ ]] || return 1
    echo "$digest"
}

# rp_registry_digest REGISTRY REPO TAG -> the manifest digest (sha256:...) the
# registry itself reports for REPO:TAG, via the registry HTTP API with the
# usual anonymous bearer-token handshake. 10 s in total. Returns 1 on any
# failure.
rp_registry_digest() {
    local reg="$1" repo="$2" tag="$3" host url hdrs challenge realm service token tokjson turl sep left start="$SECONDS"
    host="$reg"
    [ "$reg" != "docker.io" ] || host="registry-1.docker.io"
    url="https://$host/v2/$repo/manifests/$tag"
    left=$((RP_PROBE_TIMEOUT - (SECONDS - start)))
    hdrs="$(curl -sSI --connect-timeout "$RP_CONNECT_TIMEOUT" --max-time "$left" \
        -H "Accept: $RP_MANIFEST_ACCEPT" "$url" 2>/dev/null | tr -d '\r')" || return 1
    rp_header_digest "$hdrs" && return 0

    # 401: follow the Bearer challenge, scope URL-encoded.
    challenge="$(printf '%s\n' "$hdrs" | grep -i '^www-authenticate:' | head -n1)"
    realm="$(printf '%s' "$challenge" | sed -n 's/.*[Rr]ealm="\([^"]*\)".*/\1/p')"
    service="$(printf '%s' "$challenge" | sed -n 's/.*[Ss]ervice="\([^"]*\)".*/\1/p')"
    case "$realm" in https://*) ;; *) return 1 ;; esac
    sep="?"
    case "$realm" in *\?*) sep="&" ;; esac
    turl="${realm}${sep}scope=repository%3A${repo//\//%2F}%3Apull"
    [ -z "$service" ] || turl="${turl}&service=${service}"
    left=$((RP_PROBE_TIMEOUT - (SECONDS - start)))
    [ "$left" -ge 1 ] || return 1
    tokjson="$(curl -fsS --connect-timeout "$RP_CONNECT_TIMEOUT" --max-time "$left" "$turl" 2>/dev/null)" || return 1
    token="$(printf '%s' "$tokjson" | sed -n 's/.*"token"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')"
    [ -n "$token" ] || token="$(printf '%s' "$tokjson" | sed -n 's/.*"access_token"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')"
    [ -n "$token" ] || return 1
    left=$((RP_PROBE_TIMEOUT - (SECONDS - start)))
    [ "$left" -ge 1 ] || return 1
    hdrs="$(curl -sSI --connect-timeout "$RP_CONNECT_TIMEOUT" --max-time "$left" \
        -H "Accept: $RP_MANIFEST_ACCEPT" -H "Authorization: Bearer $token" "$url" 2>/dev/null | tr -d '\r')" || return 1
    rp_header_digest "$hdrs"
}

# The digest of the image as told by a TRUSTED registry (the first two entries
# of its family list, in order). Run rp_split_image first. Prints the digest and
# returns 0, or returns 1.
rp_trusted_digest() {
    local idx reg digest
    local -a regs=()
    case "$RP_IMG_REG" in
        ghcr.io) regs=("${RP_GHCR_REGISTRIES[@]}") ;;
        docker.io) regs=("${RP_HUB_REGISTRIES[@]}") ;;
        *) return 1 ;;
    esac
    for ((idx = 0; idx < RP_TRUSTED_COUNT && idx < ${#regs[@]}; idx++)); do
        reg="${regs[$idx]}"
        if digest="$(rp_registry_digest "$reg" "$RP_IMG_REPO" "$RP_IMG_TAG")"; then
            echo "$digest"
            return 0
        fi
    done
    return 1
}

# A name for messages: index 0 is the direct registry, index 1 the trusted
# relay ("mirror", never its address), the rest by their host.
rp_registry_label() { # INDEX HOST
    case "$1" in
        0) echo "direct" ;;
        1) echo "mirror" ;;
        *) echo "$2" ;;
    esac
}

# Pull IMAGE, from the first route that has it, and leave it under the name
# IMAGE (compose then finds it locally and does not pull again).
#
#  - A TRUSTED route (direct registry, the trusted relay) is pulled by tag.
#  - A PUBLIC route is pulled ONLY by digest (<mirror>/<repo>@sha256:<digest>),
#    the digest coming from a trusted registry, so Docker rejects any content
#    that is not exactly that image. If no trusted registry gives a digest,
#    public routes are skipped for this image.
#
# Each candidate is probed first (manifest, 10 s); the first that answers is
# pulled in full, and a pull is not abandoned halfway for another route. Only
# if that pull FAILS is the next candidate tried.
#   0 pulled   1 no route could deliver it
rp_pull_image() {
    local img="$1" idx reg cand ref digest="" digest_asked=0 family repo tag by_digest seen label quiet
    rp_split_image "$img"
    family="$RP_IMG_REG"
    repo="$RP_IMG_REPO"
    tag="$RP_IMG_TAG"
    while read -r idx reg cand; do
        ref="$cand"
        by_digest=0
        label="$(rp_registry_label "$idx" "$reg")"
        quiet=""
        # docker prints the full image name when it finishes; keep the relay's
        # address out of the output.
        [ "$idx" -ne 1 ] || quiet="-q"
        if [ "$idx" -ge "$RP_TRUSTED_COUNT" ]; then
            if [ "$digest_asked" -eq 0 ]; then
                digest_asked=1
                digest="$(rp_trusted_digest </dev/null)" || digest=""
                if [ -z "$digest" ]; then
                    rp_say "no trusted registry gave the digest of $img; public registries are not used for it"
                fi
            fi
            if [ -z "$digest" ]; then
                continue
            fi
            ref="$reg/$repo@$digest"
            by_digest=1
        fi
        if ! rp_manifest_ok "$ref" </dev/null; then
            rp_say "$img via $label: not reachable, trying the next registry"
            continue
        fi
        rp_say "pulling $img via $label"
        if docker pull $quiet "$ref" </dev/null; then
            if [ "$by_digest" -eq 1 ]; then
                # Docker has already checked the content against the digest;
                # this only catches an engine that tagged something else.
                seen="$(docker image inspect --format '{{json .RepoDigests}}' "$ref" 2>/dev/null </dev/null)" || seen=""
                if [ -n "$seen" ] && ! printf '%s' "$seen" | grep -qF "${digest#sha256:}"; then
                    rp_say "$img via $label does not carry the expected digest, rejected"
                    docker rmi "$ref" >/dev/null 2>&1 </dev/null || true
                    continue
                fi
            fi
            if [ "$ref" != "$img" ]; then
                if ! docker tag "$ref" "$img" </dev/null; then
                    rp_say "could not tag the image from $label, trying the next registry"
                    continue
                fi
                # Drop the mirror's name; the image itself stays under $img.
                docker rmi "$ref" >/dev/null 2>&1 </dev/null || true
            fi
            case "$family" in
                ghcr.io) RP_GHCR_OK="$idx" ;;
                docker.io) RP_HUB_OK="$idx" ;;
            esac
            return 0
        fi
        rp_say "pull of $img via $label failed, trying the next registry"
    done < <(rp_image_routes "$img")
    return 1
}

# Pull the images a compose invocation would use — only the services enabled by
# the given profiles — through rp_pull_image.
#   rp_pull_compose_images -f docker-compose.release.yaml [--profile x ...]
#   0 all pulled   1 an image could not be pulled
#   2 the image list could not be determined (the caller falls back to
#     `docker compose pull`)
rp_pull_compose_images() {
    local images img
    images="$(docker compose "$@" config --images 2>/dev/null | sort -u)" || return 2
    [ -n "$images" ] || return 2
    while IFS= read -r img; do
        [ -n "$img" ] || continue
        rp_pull_image "$img" || {
            rp_say "could not pull $img from any registry"
            return 1
        }
    done <<<"$images"
    return 0
}
