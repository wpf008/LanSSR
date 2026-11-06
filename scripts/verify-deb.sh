#!/bin/sh
# Inspect a real package; optional installation is restricted to a container.
set -eu

usage() {
    echo "Usage: $0 PACKAGE.deb [--release-directory DIRECTORY] [--install-smoke]" >&2
    echo "--release-directory compares packaged executables with the supplied release directory." >&2
    echo "--install-smoke requires root inside an isolated Docker/Podman container." >&2
    exit 2
}
[ "$#" -ge 1 ] || usage
package_argument=$1
shift
install_smoke=false
release_directory=
while [ "$#" -gt 0 ]; do
    case "$1" in
        --install-smoke) install_smoke=true; shift ;;
        --release-directory)
            [ "$#" -ge 2 ] && [ -n "$2" ] || usage
            [ -d "$2" ] || {
                echo "Release directory does not exist: $2" >&2
                exit 1
            }
            release_directory=$(cd "$2" && pwd)
            shift 2
            ;;
        *) usage ;;
    esac
done
project_root=$(cd "$(dirname "$0")/.." && pwd)
[ "$(uname -s)" = Linux ] || {
    echo "Package verification requires Linux." >&2
    exit 1
}
for command in dpkg-deb dpkg readelf desktop-file-validate cmp; do
    command -v "$command" >/dev/null 2>&1 || {
        echo "Required command is missing: $command" >&2
        exit 1
    }
done
[ -f "$package_argument" ] || { echo "Package does not exist: $package_argument" >&2; exit 1; }
package_path=$(cd "$(dirname "$package_argument")" && pwd)/$(basename "$package_argument")
[ "$(dpkg-deb --field "$package_path" Package)" = lanbridge ] || {
    echo "Expected the lanbridge package." >&2
    exit 1
}
architecture=$(dpkg-deb --field "$package_path" Architecture)
package_version=$(dpkg-deb --field "$package_path" Version)
case "$architecture" in
    arm64) expected_machine=AArch64 ;;
    amd64) expected_machine='Advanced Micro Devices X86-64' ;;
    *) echo "Unsupported package architecture: $architecture" >&2; exit 1 ;;
esac
echo "Package: $(dpkg-deb --field "$package_path" Package Version Architecture)"
echo "Depends: $(dpkg-deb --field "$package_path" Depends)"
work_dir=$(mktemp -d "${TMPDIR:-/tmp}/lanbridge-verify.XXXXXX")
trap 'rm -rf "$work_dir"' EXIT HUP INT TERM
dpkg-deb --extract "$package_path" "$work_dir/files"
dpkg-deb --control "$package_path" "$work_dir/control"
root=$work_dir/files
[ ! -e "$work_dir/control/conffiles" ] || [ ! -s "$work_dir/control/conffiles" ]
[ ! -e "$root/etc/lanbridge/imgBridge.properties" ]
[ ! -e "$root/usr/lib/lanbridge/conf" ] && [ ! -L "$root/usr/lib/lanbridge/conf" ]
[ -s "$root/usr/share/doc/lanbridge/copyright" ]
[ -s "$root/usr/share/doc/lanbridge/rfd-LICENSE" ]
[ -s "$root/usr/share/doc/lanbridge/egui_glow-LICENSE" ]
[ -s "$root/usr/share/doc/lanbridge/libwayshot-xcap-LICENSE" ]
[ -s "$root/usr/share/doc/lanbridge/xcap-LICENSE" ]
[ -s "$root/usr/share/doc/lanbridge/glutin-winit-LICENSE" ]
[ -s "$root/usr/share/doc/lanbridge/README.zh-CN.md" ]
[ -x "$root/usr/lib/lanbridge/desktop-shortcuts" ]
[ -x "$work_dir/control/postinst" ] && [ -x "$work_dir/control/prerm" ]
cmp -s "$project_root/packaging/linux/desktop-shortcuts.sh" "$root/usr/lib/lanbridge/desktop-shortcuts"
cmp -s "$project_root/packaging/linux/postinst" "$work_dir/control/postinst"
cmp -s "$project_root/packaging/linux/prerm" "$work_dir/control/prerm"
sh -n "$root/usr/lib/lanbridge/desktop-shortcuts" "$work_dir/control/postinst" "$work_dir/control/prerm"
cmp -s "$project_root/LICENSE" "$root/usr/share/doc/lanbridge/copyright" || {
    echo "Packaged project license differs from LICENSE." >&2
    exit 1
}
cmp -s "$project_root/vendor/rfd/LICENSE" "$root/usr/share/doc/lanbridge/rfd-LICENSE" || {
    echo "Packaged rfd license differs from vendor/rfd/LICENSE." >&2
    exit 1
}
cmp -s "$project_root/vendor/egui_glow/LICENSE-MIT" \
    "$root/usr/share/doc/lanbridge/egui_glow-LICENSE" || {
    echo "Packaged egui_glow license differs from the complete MIT license." >&2
    exit 1
}
cmp -s "$project_root/vendor/libwayshot-xcap/LICENSE" \
    "$root/usr/share/doc/lanbridge/libwayshot-xcap-LICENSE" || {
    echo "Packaged libwayshot-xcap license differs from vendor/libwayshot-xcap/LICENSE." >&2
    exit 1
}
cmp -s "$project_root/vendor/xcap/LICENSE" \
    "$root/usr/share/doc/lanbridge/xcap-LICENSE" || {
    echo "Packaged xcap license differs from the complete Apache 2.0 license." >&2
    exit 1
}
cmp -s "$project_root/vendor/glutin-winit/LICENSE" \
    "$root/usr/share/doc/lanbridge/glutin-winit-LICENSE" || {
    echo "Packaged glutin-winit license differs from the complete MIT license." >&2
    exit 1
}

for binary in LanSender LanReceiver; do
    executable=$root/usr/lib/lanbridge/$binary
    [ -x "$executable" ]
    if [ -n "$release_directory" ]; then
        [ -x "$release_directory/$binary" ] || {
            echo "Missing release executable: $release_directory/$binary" >&2
            exit 1
        }
        cmp -s "$release_directory/$binary" "$executable" || {
            echo "Packaged $binary differs from $release_directory/$binary." >&2
            exit 1
        }
        echo "$binary: matches release executable byte for byte"
    fi
    [ "$(readlink "$root/usr/bin/$binary")" = "../lib/lanbridge/$binary" ]
    machine=$(LC_ALL=C readelf -h "$executable" | sed -n 's/^ *Machine: *//p')
    [ "$machine" = "$expected_machine" ] || {
        echo "$binary ELF machine is '$machine', expected '$expected_machine'." >&2
        exit 1
    }
    glibc_max=$(LC_ALL=C readelf --version-info "$executable" |
        sed -n 's/.*Name: GLIBC_\([0-9.]*\).*/\1/p' | sort -Vu | tail -n 1)
    [ -n "$glibc_max" ] || {
        echo "$binary has no detectable GLIBC symbol requirements." >&2
        exit 1
    }
    dpkg --compare-versions "$glibc_max" le 2.31 || {
        echo "$binary requires GLIBC $glibc_max, newer than the 2.31 baseline." >&2
        exit 1
    }
    echo "$binary: $machine; highest required GLIBC symbol version $glibc_max"
    LC_ALL=C readelf -d "$executable" |
        sed -n 's/.*Shared library: \[\([^]]*\)\].*/  DT_NEEDED: \1/p'
done
for application in lansender lanreceiver; do
    desktop_file=$root/usr/share/applications/$application.desktop
    [ -s "$root/usr/share/icons/hicolor/scalable/apps/$application.svg" ]
    desktop-file-validate "$desktop_file"
    [ "$(sed -n 's/^Terminal=//p' "$desktop_file")" = false ]
    [ "$(sed -n 's/^Icon=//p' "$desktop_file")" = "$application" ]
    case "$application" in
        lansender) executable=LanSender ;;
        lanreceiver) executable=LanReceiver ;;
    esac
    [ "$(sed -n 's/^Exec=//p' "$desktop_file")" = /usr/bin/$executable ]
    [ "$(sed -n 's/^Name=//p' "$desktop_file")" = "$executable" ]
    [ "$(sed -n 's/^Name\[zh_CN\]=//p' "$desktop_file")" = "$executable" ]
    ! grep -q '^GenericName' "$desktop_file"
    grep -qx 'X-LanSSR-Desktop-Shortcut=true' "$desktop_file"
done
echo "Package structure, licenses, launchers, built-in settings and ELF baseline verified."

if "$install_smoke"; then
    command -v ldd >/dev/null 2>&1 || {
        echo "Required command is missing: ldd" >&2
        exit 1
    }
    [ "$(id -u)" = 0 ] || {
        echo "Installation smoke test requires root in an isolated container." >&2
        exit 1
    }
    [ -f /.dockerenv ] || [ -f /run/.containerenv ] || {
        echo "Refusing installation smoke test outside Docker/Podman." >&2
        exit 1
    }
    [ "$(dpkg --print-architecture)" = "$architecture" ] || {
        echo "The container architecture does not match the package." >&2
        exit 1
    }
    dpkg -i "$package_path"
    audit_output=$(dpkg --audit lanbridge)
    [ -z "$audit_output" ] || {
        printf '%s\n' "$audit_output" >&2
        echo "Installed lanbridge package failed dpkg --audit." >&2
        exit 1
    }
    echo "Installed lanbridge package: dpkg --audit passed"
    cd "$work_dir"
    for binary in LanSender LanReceiver; do
        [ "$(command -v "$binary")" = /usr/bin/$binary ]
        [ "$(readlink -f "/usr/bin/$binary")" = /usr/lib/lanbridge/$binary ]
        LC_ALL=C ldd "/usr/lib/lanbridge/$binary" > "$work_dir/$binary.ldd" 2>&1
        case "$(cat "$work_dir/$binary.ldd")" in
            *'not found'*)
                cat "$work_dir/$binary.ldd" >&2
                echo "$binary has missing shared libraries." >&2
                exit 1
                ;;
        esac
        echo "$binary ldd: no missing shared libraries"
        actual_version=$("$binary" --version)
        [ "$actual_version" = "$binary $package_version" ] || {
            echo "$binary reports '$actual_version'; expected '$binary $package_version'." >&2
            exit 1
        }
        echo "$binary --version: matches package version $package_version"
        "$binary" --help > "$work_dir/$binary.help"
        [ -s "$work_dir/$binary.help" ]
        case "$(cat "$work_dir/$binary.help")" in
            *--config*) echo "$binary still accepts external configuration files." >&2; exit 1 ;;
        esac
        echo "$binary --help: passed"
    done
    # Validate options from another CWD without any adjacent properties file.
    # This error is reported before output-directory creation or display access.
    if LanReceiver --cli --max-bytes 0 > "$work_dir/defaults-check.log" 2>&1; then
        echo "Expected the deliberately invalid max-bytes value to fail." >&2
        exit 1
    fi
    case "$(cat "$work_dir/defaults-check.log")" in
        *'max-bytes must be positive'*) echo "Launch with built-in defaults from another CWD: passed" ;;
        *) cat "$work_dir/defaults-check.log" >&2; exit 1 ;;
    esac
    echo "Container installation smoke test passed."
fi
