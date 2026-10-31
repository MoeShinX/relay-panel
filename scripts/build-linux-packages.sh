#!/usr/bin/env bash
# Build installable Debian and RPM packages from the current checkout.
# Prerequisites: Rust (from rust-toolchain.toml), Node 20+, and nFPM v2.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

version="${VERSION:-$(awk -F '"' '/^version = / { print $2; exit }' crates/panel/Cargo.toml)}"
if ! [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "VERSION must be a stable semver (for example 1.2.18); got: $version" >&2
    exit 2
fi

if ! command -v nfpm >/dev/null 2>&1; then
    echo "nfpm v2 is required but was not found in PATH." >&2
    exit 127
fi

stage_dir="$repo_root/dist/package-root"
output_dir="$repo_root/dist/packages"
rm -rf "$stage_dir" "$output_dir"
mkdir -p "$stage_dir" "$output_dir"

npm ci --prefix frontend --no-audit --no-fund
npm run build --prefix frontend
cargo build --locked --release -p relay-panel

install -Dm755 target/release/relay-panel "$stage_dir/usr/bin/relay-panel"
install -d -m755 "$stage_dir/usr/share/relay-panel/public"
cp -a frontend/dist/. "$stage_dir/usr/share/relay-panel/public/"
install -Dm644 packaging/systemd/relaypanel.service \
    "$stage_dir/usr/lib/systemd/system/relaypanel.service"
install -Dm640 packaging/relay-panel.env \
    "$stage_dir/etc/relay-panel/relay-panel.env"

export VERSION="$version"
nfpm package --config packaging/nfpm.yaml --packager deb --target "$output_dir"
nfpm package --config packaging/nfpm.yaml --packager rpm --target "$output_dir"

sha256sum "$output_dir"/* > "$output_dir/SHA256SUMS"
printf 'Packages written to %s\n' "$output_dir"
