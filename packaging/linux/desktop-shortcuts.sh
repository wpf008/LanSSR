#!/bin/sh
# Create shortcuts as the desktop owner, never as root in a user's home.
# No graphical helper, network access, or sourced user configuration is needed.
set -eu

operation=${1:---install}
case "$operation" in
    --install-for-users|--remove-for-users)
        [ "$(id -u)" = 0 ] || {
            echo 'Desktop shortcut setup for users requires root.' >&2
            exit 1
        }
        if [ "$operation" = --install-for-users ]; then
            user_operation=--install
        else
            user_operation=--remove
        fi
        # sudo preserves the actual installing account. Graphical installers
        # normally run as root without SUDO_USER, so use existing login accounts.
        if [ "$user_operation" = --install ] && [ -n "${SUDO_USER:-}" ] &&
            [ "$SUDO_USER" != root ] && getent passwd "$SUDO_USER" >/dev/null; then
            shortcut_users=$SUDO_USER
        else
            shortcut_users=$(getent passwd | awk -F: '
                $3 >= 1000 && $3 < 60000 && $6 ~ /^\// &&
                $7 !~ /(nologin|false)$/ { print $1 }')
        fi
        for shortcut_user in $shortcut_users; do
            if command -v runuser >/dev/null 2>&1; then
                runuser -u "$shortcut_user" -- "$0" "$user_operation" ||
                    echo "Could not update desktop shortcuts for $shortcut_user." >&2
            elif command -v su >/dev/null 2>&1; then
                # The command contains no account names or home-directory data.
                su -s /bin/sh "$shortcut_user" -c "/usr/lib/lanbridge/desktop-shortcuts $user_operation" ||
                    echo "Could not update desktop shortcuts for $shortcut_user." >&2
            else
                echo 'Desktop shortcuts could not be created: runuser and su are unavailable.' >&2
            fi
        done
        exit 0
        ;;
    --install|--remove) ;;
    *) echo "Unknown desktop shortcut operation: $operation" >&2; exit 2 ;;
esac

[ "$(id -u)" != 0 ] || {
    echo 'Refusing to create desktop shortcuts as root.' >&2
    exit 1
}
[ -n "${HOME:-}" ] && [ -d "$HOME" ] || exit 0

# Read only the quoted XDG desktop value; do not evaluate arbitrary shell code
# from a user-owned config file. Standard user-dirs.dirs uses literal $HOME.
desktop_dir=
desktop_config=$HOME/.config/user-dirs.dirs
if [ -r "$desktop_config" ]; then
    desktop_value=$(sed -n 's/^XDG_DESKTOP_DIR="\(.*\)"[[:space:]]*$/\1/p' "$desktop_config" | head -n 1)
    case "$desktop_value" in
        '$HOME') desktop_dir=$HOME ;;
        '$HOME/'*) desktop_dir=$HOME/${desktop_value#\$HOME/} ;;
        /*) desktop_dir=$desktop_value ;;
    esac
fi
if [ -z "$desktop_dir" ]; then
    if [ -d "$HOME/桌面" ]; then
        desktop_dir=$HOME/桌面
    elif [ -d "$HOME/Desktop" ]; then
        desktop_dir=$HOME/Desktop
    else
        case "${LANG:-${LC_ALL:-}}" in
            zh*) desktop_dir=$HOME/桌面 ;;
            *) desktop_dir=$HOME/Desktop ;;
        esac
    fi
fi
# The XDG convention '$HOME' disables the desktop directory.
[ "$desktop_dir" != "$HOME" ] || exit 0
if [ "$operation" = --install ]; then
    mkdir -p "$desktop_dir"
else
    [ -d "$desktop_dir" ] || exit 0
fi
shortcut_state=$HOME/.local/share/lanbridge/desktop-shortcuts
if [ "$operation" = --install ]; then
    mkdir -p "$shortcut_state"
fi

for application in LanSender LanReceiver; do
    case "$application" in
        LanSender) menu_file=/usr/share/applications/lansender.desktop ;;
        LanReceiver) menu_file=/usr/share/applications/lanreceiver.desktop ;;
    esac
    shortcut=$desktop_dir/$application.desktop
    saved_shortcut=$shortcut_state/$application.desktop
    if [ -e "$shortcut" ] || [ -L "$shortcut" ]; then
        # Preserve unrelated user-created files and any symlink at this name.
        if [ -L "$shortcut" ] || ! grep -qx 'X-LanSSR-Desktop-Shortcut=true' "$shortcut"; then
            echo "Preserving existing desktop file: $shortcut" >&2
            continue
        fi
        if [ -f "$saved_shortcut" ] && ! cmp -s "$shortcut" "$saved_shortcut"; then
            echo "Preserving customized desktop shortcut: $shortcut" >&2
            continue
        fi
    elif [ "$operation" = --remove ]; then
        continue
    fi
    if [ "$operation" = --remove ]; then
        rm -f "$shortcut"
        rm -f "$saved_shortcut"
        continue
    fi
    install -m 755 "$menu_file" "$shortcut"
    install -m 644 "$menu_file" "$saved_shortcut"
    # UKUI/GNOME desktops may use GVFS trusted metadata as well as executable
    # permission. This is best effort; it must not add an installation dependency.
    user_runtime=/run/user/$(id -u)
    unset XDG_RUNTIME_DIR DBUS_SESSION_BUS_ADDRESS
    if [ -d "$user_runtime" ]; then
        XDG_RUNTIME_DIR=$user_runtime
        export XDG_RUNTIME_DIR
        if [ -S "$user_runtime/bus" ]; then
            DBUS_SESSION_BUS_ADDRESS=unix:path=$user_runtime/bus
            export DBUS_SESSION_BUS_ADDRESS
        fi
    fi
    if command -v gio >/dev/null 2>&1 && command -v timeout >/dev/null 2>&1; then
        timeout 3s gio set "$shortcut" metadata::trusted true >/dev/null 2>&1 || true
    fi
done
if [ "$operation" = --remove ] && [ -d "$shortcut_state" ]; then
    rmdir "$shortcut_state" >/dev/null 2>&1 || true
fi
