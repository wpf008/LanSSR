#!/bin/sh
# Reproduce an offline desktop without optional file-dialog helper packages.
# Run only in a disposable ARM64 container, normally with --network none.
set -eu
cd "$(dirname "$0")/.."

if [ "$#" -lt 1 ] || [ "$#" -gt 2 ]; then
    echo 'Usage: sh scripts/verify-kylin-offline.sh NEW.deb [OLD.deb]' >&2
    exit 2
fi
[ -f /.dockerenv ] || [ -f /run/.containerenv ] || {
    echo 'Refusing to remove helper packages outside a disposable container.' >&2
    exit 1
}
[ "$(id -u)" = 0 ] && [ "$(dpkg --print-architecture)" = arm64 ] || {
    echo 'This check requires root in an ARM64 container.' >&2
    exit 1
}
new_package=$1
[ -f "$new_package" ] || { echo "Missing package: $new_package" >&2; exit 1; }
new_depends=$(dpkg-deb -f "$new_package" Depends)
case "$new_depends" in
    *zenity*) echo 'The new package still requires zenity.' >&2; exit 1 ;;
esac

dpkg -r zenity xdg-desktop-portal-gtk xdg-desktop-portal
if command -v zenity >/dev/null 2>&1; then
    echo 'zenity is still available.' >&2
    exit 1
fi
for helper in zenity xdg-desktop-portal xdg-desktop-portal-gtk; do
    helper_state=$(dpkg-query -W -f='${db:Status-Status}' "$helper" 2>/dev/null || true)
    [ "$helper_state" != installed ] || {
        echo "$helper is still installed." >&2
        exit 1
    }
done
echo 'Confirmed: zenity and desktop portals are absent.'

if [ "$#" = 2 ]; then
    old_package=$2
    [ -f "$old_package" ] || { echo "Missing old package: $old_package" >&2; exit 1; }
    expected_version=$(dpkg-deb -f "$old_package" Version)
    case "$(dpkg-deb -f "$old_package" Depends)" in
        *zenity*)
            failure_log=$(mktemp /tmp/lanbridge-old-deb.XXXXXX)
            trap 'rm -f "$failure_log"' EXIT HUP INT TERM
            if LC_ALL=C dpkg -i "$old_package" > "$failure_log" 2>&1; then
                echo 'The old package unexpectedly installed without zenity.' >&2
                exit 1
            fi
            cat "$failure_log"
            case "$(cat "$failure_log")" in
                *'depends on zenity'*) ;;
                *) echo 'The old package did not reproduce the zenity dependency error.' >&2; exit 1 ;;
            esac
            [ "$(dpkg-query -W -f='${Status}' lanbridge)" = 'install ok unpacked' ]
            echo 'Reproduced: old package unpacked but unconfigured due to missing zenity.'
            ;;
        *)
            dpkg -i "$old_package"
            [ "$(dpkg-query -W -f='${Status}' lanbridge)" = 'install ok installed' ]
            echo 'Old package installed successfully for a direct offline upgrade.'
            ;;
    esac
    [ "$(dpkg-query -W -f='${Version}' lanbridge)" = "$expected_version" ]
fi

sh scripts/verify-deb.sh "$new_package" \
    --release-directory target/linux-arm64/release --install-smoke
echo 'Passed: offline installation without zenity or desktop portals.'
