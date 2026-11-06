#!/bin/sh
# Assemble a Debian package from already-built, architecture-matched Linux ELFs.
set -eu
cd "$(dirname "$0")/.."

usage() {
    echo "Usage: $0 BINARY_DIRECTORY arm64|amd64 [OUTPUT_DIRECTORY]" >&2
    echo "Run inside the Linux build environment with dpkg-dev and desktop-file-utils installed." >&2
    exit 2
}
[ "$#" -ge 2 ] && [ "$#" -le 3 ] || usage
[ "$(uname -s)" = Linux ] || {
    echo "This script requires Linux to resolve the binaries' Debian dependencies." >&2
    exit 1
}
binary_dir=$(cd "$1" && pwd)
architecture=$2
case "$architecture" in
    arm64) expected_machine=AArch64 ;;
    amd64) expected_machine='Advanced Micro Devices X86-64' ;;
    *) usage ;;
esac
output_dir=${3:-dist/linux-$architecture}
mkdir -p "$output_dir"
output_dir=$(cd "$output_dir" && pwd)
version=${LANBRIDGE_VERSION:-$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -n 1)}
case "$version" in
    ''|*[!0-9A-Za-z.+:~_-]*) echo "Invalid package version: $version" >&2; exit 2 ;;
esac
for command in dpkg-deb dpkg-shlibdeps readelf desktop-file-validate; do
    command -v "$command" >/dev/null 2>&1 || {
        echo "Required command is missing: $command" >&2
        exit 1
    }
done
for binary in LanSender LanReceiver; do
    [ -x "$binary_dir/$binary" ] || {
        echo "Missing executable: $binary_dir/$binary" >&2
        exit 1
    }
    machine=$(LC_ALL=C readelf -h "$binary_dir/$binary" | sed -n 's/^ *Machine: *//p')
    [ "$machine" = "$expected_machine" ] || {
        echo "$binary has machine '$machine'; expected '$expected_machine'." >&2
        exit 1
    }
done
work_dir=$(mktemp -d "${TMPDIR:-/tmp}/lanbridge-deb.XXXXXX")
trap 'rm -rf "$work_dir"' EXIT HUP INT TERM
package_root=$work_dir/package
mkdir -p "$package_root/DEBIAN" "$package_root/usr/lib/lanbridge" \
    "$package_root/usr/bin" \
    "$package_root/usr/share/applications" \
    "$package_root/usr/share/icons/hicolor/scalable/apps" \
    "$package_root/usr/share/doc/lanbridge" "$work_dir/debian"

for binary in LanSender LanReceiver; do
    install -m 755 "$binary_dir/$binary" "$package_root/usr/lib/lanbridge/$binary"
    ln -s "../lib/lanbridge/$binary" "$package_root/usr/bin/$binary"
done
install -m 755 packaging/linux/desktop-shortcuts.sh "$package_root/usr/lib/lanbridge/desktop-shortcuts"
install -m 755 packaging/linux/postinst packaging/linux/prerm "$package_root/DEBIAN/"
install -m 644 packaging/linux/lansender.desktop packaging/linux/lanreceiver.desktop \
    "$package_root/usr/share/applications/"
install -m 644 packaging/linux/lansender.svg packaging/linux/lanreceiver.svg \
    "$package_root/usr/share/icons/hicolor/scalable/apps/"
install -m 644 LICENSE "$package_root/usr/share/doc/lanbridge/copyright"
install -m 644 vendor/rfd/LICENSE "$package_root/usr/share/doc/lanbridge/rfd-LICENSE"
install -m 644 vendor/egui_glow/LICENSE-MIT "$package_root/usr/share/doc/lanbridge/egui_glow-LICENSE"
install -m 644 vendor/libwayshot-xcap/LICENSE "$package_root/usr/share/doc/lanbridge/libwayshot-xcap-LICENSE"
install -m 644 vendor/xcap/LICENSE "$package_root/usr/share/doc/lanbridge/xcap-LICENSE"
install -m 644 vendor/glutin-winit/LICENSE "$package_root/usr/share/doc/lanbridge/glutin-winit-LICENSE"
install -m 644 packaging/linux/README.zh-CN.md "$package_root/usr/share/doc/lanbridge/README.zh-CN.md"
desktop-file-validate "$package_root/usr/share/applications/lansender.desktop" \
    "$package_root/usr/share/applications/lanreceiver.desktop"

# dpkg-shlibdeps requires a Debian source control file. Resolve library versions
# against the actual build environment instead of guessing a dependency baseline.
cat > "$work_dir/debian/control" <<CONTROL
Source: lanbridge
Section: utils
Priority: optional
Maintainer: LanBridge Packagers <packages@example.invalid>

Package: lanbridge
Architecture: $architecture
Description: Screen-based file transfer desktop clients
CONTROL
shlibs_output=$(cd "$work_dir" && dpkg-shlibdeps -O \
    -e"$package_root/usr/lib/lanbridge/LanSender" \
    -e"$package_root/usr/lib/lanbridge/LanReceiver")
depends=$(printf '%s\n' "$shlibs_output" | sed -n 's/^shlibs:Depends=//p')
[ -n "$depends" ] || {
    echo "No shared-library dependency information was generated." >&2
    exit 1
}
# The window/rendering backends load some libraries at runtime. File and
# directory selection are built into the Linux UI and need no external helper.
# xdg-utils opens received-file locations; portals are optional for Wayland capture.
for runtime in libgl1 libegl1 libx11-6 libx11-xcb1 libxcursor1 libxi6 libxrandr2 \
    libxkbcommon-x11-0 libwayland-client0 libwayland-cursor0 libwayland-egl1 \
    xdg-utils; do
    case ", $depends, " in
        *", $runtime,"*|*", $runtime ("*) ;;
        *) depends="$depends, $runtime" ;;
    esac
done
depends="$depends, fonts-noto-cjk | fonts-wqy-microhei"
installed_size=$(du -sk "$package_root/usr" | awk '{size += $1} END {print size}')
cat > "$package_root/DEBIAN/control" <<CONTROL
Package: lanbridge
Version: $version
Architecture: $architecture
Section: utils
Priority: optional
Maintainer: LanBridge Packagers <packages@example.invalid>
Installed-Size: $installed_size
Depends: $depends
Suggests: xdg-desktop-portal, xdg-desktop-portal-gtk | xdg-desktop-portal-kde | xdg-desktop-portal-backend
Description: LanSender and LanReceiver screen-based file transfer clients
 Native desktop clients for sending and continuously receiving files using
 encoded screen images. Includes Chinese controls, dark/light themes,
 receive history, configurable output directory, and desktop launchers.
CONTROL
find "$package_root" -type d -exec chmod 755 {} +
chmod 644 "$package_root/DEBIAN/control"

# Keep the installed Debian identity for seamless upgrades from lanbridge 2.0.x.
# Only the downloadable filename uses the product name requested for this release.
package_path=$output_dir/LanSSR_${version}_${architecture}.deb
# gzip is understood by the older dpkg releases shipped with Kylin V10.
dpkg-deb --root-owner-group -Zgzip --build "$package_root" "$package_path"
cp packaging/linux/README.zh-CN.md "$output_dir/使用说明.md"
dpkg-deb --info "$package_path"
echo "Packaged: $package_path"
