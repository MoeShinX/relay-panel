#!/usr/bin/env bash
# Build installable Debian and RPM packages from the current checkout.
# Prerequisites: Rust (from rust-toolchain.toml), Node 20+, and nFPM v2.
# TARGET may be any supported Rust Linux target; CROSS_LINKER is required for
# non-native targets and should name that target's GNU C compiler.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

# A release tag on HEAD is authoritative. Development packages are tied to
# their exact source commit while remaining valid Debian/RPM version strings.
if [ -n "${VERSION:-}" ]; then
    version="$VERSION"
else
    release_tag="$(git tag --points-at HEAD --list 'v[0-9]*' | sort -V | tail -n 1)"
    if [ -n "$release_tag" ]; then
        version="${release_tag#v}"
    else
        version="0.0.0+git.$(git rev-parse --short=12 HEAD)"
    fi
fi
if ! [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+([+][0-9A-Za-z.]+)?$ ]]; then
    echo "VERSION must be a release semver or commit version; got: $version" >&2
    exit 2
fi

if ! command -v nfpm >/dev/null 2>&1; then
    echo "nfpm v2 is required but was not found in PATH." >&2
    exit 127
fi

target="${TARGET:-x86_64-unknown-linux-gnu}"
case "$target" in
    x86_64-unknown-linux-gnu) arch=amd64 ;;
    aarch64-unknown-linux-gnu) arch=arm64 ;;
    powerpc64le-unknown-linux-gnu) arch=ppc64le ;;
    riscv64gc-unknown-linux-gnu) arch=riscv64 ;;
    s390x-unknown-linux-gnu) arch=s390x ;;
    *)
        echo "Unsupported TARGET: $target" >&2
        exit 2
        ;;
esac

stage_dir="$repo_root/dist/package-root/$target"
output_dir="$repo_root/dist/packages/$target"
rm -rf "$stage_dir" "$output_dir"
mkdir -p "$stage_dir" "$output_dir"

if [ "${SKIP_FRONTEND_BUILD:-0}" != 1 ]; then
    npm ci --prefix frontend --no-audit --no-fund
    npm run build --prefix frontend
fi

if [ ! -f frontend/dist/index.html ]; then
    echo "frontend/dist is missing; build the frontend or unset SKIP_FRONTEND_BUILD." >&2
    exit 1
fi

binary_path="target/release/relay-panel"
if [ "$target" = x86_64-unknown-linux-gnu ]; then
    cargo build --locked --release -p relay-panel
else
    : "${CROSS_LINKER:?CROSS_LINKER is required when TARGET is not native}"
    linker_var="CARGO_TARGET_$(printf '%s' "$target" | tr '[:lower:]-' '[:upper:]_')_LINKER"
    cc_var="CC_$(printf '%s' "$target" | tr '-' '_')"
    ar_var="AR_$(printf '%s' "$target" | tr '-' '_')"
    export "$linker_var=$CROSS_LINKER"
    export "$cc_var=$CROSS_LINKER"
    export "$ar_var=${CROSS_LINKER%-gcc}-ar"
    cargo build --locked --release --target "$target" -p relay-panel
    binary_path="target/$target/release/relay-panel"
fi

install -Dm755 "$binary_path" "$stage_dir/usr/bin/relay-panel"
install -d -m755 "$stage_dir/usr/share/relay-panel/public"
cp -a frontend/dist/. "$stage_dir/usr/share/relay-panel/public/"
install -Dm644 packaging/systemd/relaypanel.service \
    "$stage_dir/usr/lib/systemd/system/relaypanel.service"
install -Dm640 packaging/relay-panel.env \
    "$stage_dir/etc/relay-panel/relay-panel.env"

export VERSION="$version"
export ARCH="$arch"
nfpm package --config packaging/nfpm.yaml --packager deb --target "$output_dir"
nfpm package --config packaging/nfpm.yaml --packager rpm --target "$output_dir"

sha256sum "$output_dir"/* > "$output_dir/SHA256SUMS"
printf 'Packages written to %s\n' "$output_dir"
