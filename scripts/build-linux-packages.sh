#!/usr/bin/env bash
# Build installable Debian and RPM packages from the current checkout.
# Prerequisites: Rust (from rust-toolchain.toml), Node 20+, dpkg-deb, and/or
# rpmbuild. Packages are built with the native Debian/RPM tools, not nFPM.
# Supported targets are built on matching native GitHub runners; cross builds
# are intentionally not supported by this packaging script.
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

package_format="${PACKAGE_FORMAT:-all}"
case "$package_format" in
    deb|rpm|all) ;;
    *) echo "PACKAGE_FORMAT must be deb, rpm, or all; got: $package_format" >&2; exit 2 ;;
esac
if { [ "$package_format" = deb ] || [ "$package_format" = all ]; } && ! command -v dpkg-deb >/dev/null 2>&1; then
    echo "dpkg-deb is required to build a DEB package." >&2
    exit 127
fi
if { [ "$package_format" = rpm ] || [ "$package_format" = all ]; } && ! command -v rpmbuild >/dev/null 2>&1; then
    echo "rpmbuild is required to build an RPM package." >&2
    exit 127
fi

target="${TARGET:-x86_64-unknown-linux-gnu}"
case "$target" in
    x86_64-unknown-linux-gnu) deb_arch=amd64; rpm_arch=x86_64 ;;
    aarch64-unknown-linux-gnu) deb_arch=arm64; rpm_arch=aarch64 ;;
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
skip_build="${SKIP_BUILD:-0}"

if [ "$skip_build" != 1 ]; then
    rm -rf "$stage_dir" "$deb_root" "$rpm_topdir" "$output_dir"
    mkdir -p "$stage_dir" "$output_dir"

    if [ "${SKIP_FRONTEND_BUILD:-0}" != 1 ]; then
        npm ci --prefix frontend --no-audit --no-fund
        npm run build --prefix frontend
    fi

    if [ ! -f frontend/dist/index.html ]; then
        echo "frontend/dist is missing; build the frontend or unset SKIP_FRONTEND_BUILD." >&2
        exit 1
    fi

    # Keep package binaries compact. Cargo.toml already supplies strip=true,
    # fat LTO, and one codegen unit for the release profile; these two
    # package-only overrides favor size and avoid unwinding runtime support.
    export CARGO_PROFILE_RELEASE_OPT_LEVEL="${CARGO_PROFILE_RELEASE_OPT_LEVEL:-z}"
    export CARGO_PROFILE_RELEASE_PANIC="${CARGO_PROFILE_RELEASE_PANIC:-abort}"
    cargo build --locked --release -p relay-panel
    binary_path="target/release/relay-panel"

    install -Dm755 "$binary_path" "$stage_dir/usr/bin/relay-panel"
    install -d -m755 "$stage_dir/usr/share/relay-panel/public"
    cp -a frontend/dist/. "$stage_dir/usr/share/relay-panel/public/"
    install -Dm644 packaging/systemd/relaypanel.service \
        "$stage_dir/usr/lib/systemd/system/relaypanel.service"
    install -Dm640 packaging/relay-panel.env \
        "$stage_dir/etc/relay-panel/relay-panel.env"
elif [ ! -f "$stage_dir/usr/bin/relay-panel" ]; then
    echo "Package staging directory is missing; run without SKIP_BUILD first." >&2
    exit 1
fi

if [ "$package_format" = deb ] || [ "$package_format" = all ]; then
    # Debian package: control.tar.* is created from this explicit control file.
    rm -rf "$deb_root"
    mkdir -p "$deb_root/DEBIAN"
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
fi

if [ "$package_format" = rpm ] || [ "$package_format" = all ]; then
    # RPM package: generate a spec file with complete metadata and use rpmbuild.
    rm -rf "$rpm_topdir"
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
fi

(
    cd "$output_dir"
    find . -maxdepth 1 -type f ! -name SHA256SUMS -print0 \
        | sort -z \
        | xargs -0 sha256sum
) > "$output_dir/SHA256SUMS"
printf 'Packages written to %s\n' "$output_dir"
