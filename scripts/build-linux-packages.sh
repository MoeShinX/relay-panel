#!/usr/bin/env bash
# Build installable Debian and RPM packages from the current checkout.
# Prerequisites: Rust (from rust-toolchain.toml), Node 20+, dpkg-deb, and
# rpmbuild. Packages are built with the native Debian/RPM tools, not nFPM.
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

if ! command -v dpkg-deb >/dev/null 2>&1 || ! command -v rpmbuild >/dev/null 2>&1; then
    echo "dpkg-deb and rpmbuild are required but were not found in PATH." >&2
    exit 127
fi

target="${TARGET:-x86_64-unknown-linux-gnu}"
case "$target" in
    x86_64-unknown-linux-gnu) deb_arch=amd64; rpm_arch=x86_64 ;;
    aarch64-unknown-linux-gnu) deb_arch=arm64; rpm_arch=aarch64 ;;
    powerpc64le-unknown-linux-gnu) deb_arch=ppc64el; rpm_arch=ppc64le ;;
    riscv64gc-unknown-linux-gnu) deb_arch=riscv64; rpm_arch=riscv64 ;;
    s390x-unknown-linux-gnu) deb_arch=s390x; rpm_arch=s390x ;;
    *)
        echo "Unsupported TARGET: $target" >&2
        exit 2
        ;;
esac

# Each GitHub Actions matrix job has its own workspace, so a shared staging
# path is safe there. Local builds also clean it before use.
stage_dir="$repo_root/dist/package-root"
output_dir="$repo_root/dist/packages/$target"
deb_root="$repo_root/dist/deb-root"
rpm_topdir="$repo_root/dist/rpmbuild/$target"
rm -rf "$stage_dir" "$deb_root" "$rpm_topdir" "$output_dir"
mkdir -p "$stage_dir" "$deb_root/DEBIAN" "$output_dir"

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

# Debian package: control.tar.* is created by dpkg-deb from this explicit
# DEBIAN/control file. Do not replace this with an opaque third-party packer.
cp -a "$stage_dir/." "$deb_root/"
cat > "$deb_root/DEBIAN/control" <<EOF
Package: relay-panel
Version: $version
Section: net
Priority: optional
Architecture: $deb_arch
Maintainer: MoeShinX
Depends: ca-certificates
Description: Self-hosted TCP/UDP forwarding management panel
 RelayPanel provides a web UI for forwarding rules, relay nodes, and traffic
 statistics.
EOF
cp packaging/scripts/preinstall.sh "$deb_root/DEBIAN/preinst"
cp packaging/scripts/postinstall.sh "$deb_root/DEBIAN/postinst"
cp packaging/scripts/postremove.sh "$deb_root/DEBIAN/postrm"
chmod 0755 "$deb_root/DEBIAN/preinst" "$deb_root/DEBIAN/postinst" "$deb_root/DEBIAN/postrm"
dpkg-deb --root-owner-group --build "$deb_root" \
    "$output_dir/relay-panel_${version}_${deb_arch}.deb"

# RPM package: generate a spec file with complete metadata and use rpmbuild.
mkdir -p "$rpm_topdir/BUILD" "$rpm_topdir/BUILDROOT" "$rpm_topdir/RPMS" \
    "$rpm_topdir/SOURCES" "$rpm_topdir/SPECS" "$rpm_topdir/SRPMS"
spec_file="$rpm_topdir/SPECS/relay-panel.spec"
cat > "$spec_file" <<EOF
Name:           relay-panel
Version:        $version
Release:        1%{?dist}
Summary:        Self-hosted TCP/UDP forwarding management panel
License:        Apache-2.0
URL:            https://github.com/MoeShinX/relay-panel
BuildArch:      $rpm_arch
Requires:       ca-certificates

%description
RelayPanel provides a web UI for forwarding rules, relay nodes, and traffic
statistics.

%prep

%build

%install
rm -rf %{buildroot}
cp -a $stage_dir/. %{buildroot}/

%pre
EOF
cat packaging/scripts/preinstall.sh >> "$spec_file"
cat >> "$spec_file" <<'EOF'

%post
EOF
cat packaging/scripts/postinstall.sh >> "$spec_file"
cat >> "$spec_file" <<'EOF'

%postun
EOF
cat packaging/scripts/postremove.sh >> "$spec_file"
cat >> "$spec_file" <<'EOF'

%files
%attr(0755,root,root) /usr/bin/relay-panel
/usr/share/relay-panel/public
%attr(0644,root,root) /usr/lib/systemd/system/relaypanel.service
%config(noreplace) %attr(0640,root,relaypanel) /etc/relay-panel/relay-panel.env

%changelog
* Thu Jan 01 1970 MoeShinX <noreply@github.com> - PACKAGE_VERSION-1
- Automated package build
EOF
sed -i "s/PACKAGE_VERSION/$version/g" "$spec_file"
rpmbuild -bb "$spec_file" --define "_topdir $rpm_topdir"
cp "$rpm_topdir/RPMS/$rpm_arch"/*.rpm "$output_dir/"

sha256sum "$output_dir"/* > "$output_dir/SHA256SUMS"
printf 'Packages written to %s\n' "$output_dir"
