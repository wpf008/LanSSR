#!/bin/sh
# Exercise real maintainer scripts, only in a disposable Debian container.
set -eu
cd "$(dirname "$0")/.."
if [ "$#" -lt 1 ] || [ "$#" -gt 2 ]; then
    echo 'Usage: sh scripts/verify-desktop-shortcuts.sh NEW.deb [OLD.deb]' >&2
    exit 2
fi
[ -f /.dockerenv ] || [ -f /run/.containerenv ] || {
    echo 'Refusing shortcut installation tests outside a disposable container.' >&2
    exit 1
}
[ "$(id -u)" = 0 ] || { echo 'This test requires container root.' >&2; exit 1; }
new_package=$1
[ -f "$new_package" ] || { echo "Missing package: $new_package" >&2; exit 1; }
case "$(basename "$new_package")" in
    LanSSR_*.deb) ;;
    *) echo 'Expected the new LanSSR deb filename prefix.' >&2; exit 1 ;;
esac

for test_user in lan-shortcut-sudo lan-shortcut-gui lan-shortcut-disabled lan-shortcut-custom; do
    ! getent passwd "$test_user" >/dev/null || {
        echo "Test account already exists: $test_user" >&2
        exit 1
    }
    useradd -m -s /bin/sh "$test_user"
    install -d -m 755 -o "$test_user" -g "$test_user" "/home/$test_user/.config"
done
useradd -r -m -s /usr/sbin/nologin lan-shortcut-system
sudo_home=/home/lan-shortcut-sudo
gui_home=/home/lan-shortcut-gui
disabled_home=/home/lan-shortcut-disabled
custom_home=/home/lan-shortcut-custom
install -d -m 755 -o lan-shortcut-sudo -g lan-shortcut-sudo "$sudo_home/桌面/应用 快捷方式"
cat > "$sudo_home/.config/user-dirs.dirs" <<'CONFIG'
XDG_DESKTOP_DIR="$HOME/桌面/应用 快捷方式"
touch /tmp/lan-shortcut-config-executed
CONFIG
cat > "$gui_home/.config/user-dirs.dirs" <<'CONFIG'
XDG_DESKTOP_DIR="$HOME/Desktop"
CONFIG
cat > "$disabled_home/.config/user-dirs.dirs" <<'CONFIG'
XDG_DESKTOP_DIR="$HOME"
CONFIG
cat > "$custom_home/.config/user-dirs.dirs" <<'CONFIG'
XDG_DESKTOP_DIR="$HOME/桌面"
CONFIG
install -d -m 755 -o lan-shortcut-custom -g lan-shortcut-custom "$custom_home/桌面"
printf '%s\n' '[Desktop Entry]' 'Name=My custom Sender' > "$custom_home/桌面/LanSender.desktop"
chown lan-shortcut-custom:lan-shortcut-custom "$custom_home/桌面/LanSender.desktop"

if [ "$#" = 2 ]; then
    old_package=$2
    [ -f "$old_package" ] || { echo "Missing old package: $old_package" >&2; exit 1; }
    # Reproduce the same sudo user's install before upgrading. A graphical
    # old-package install would also create the other users' shortcuts, which
    # are deliberately checked later in the graphical-installer scenario.
    SUDO_USER=lan-shortcut-sudo dpkg -i "$old_package"
    [ "$(dpkg-query -W -f='${Status}' lanbridge)" = 'install ok installed' ]
    echo "Installed old version $(dpkg-query -W -f='${Version}' lanbridge) for direct upgrade."
fi

verify_shortcuts() {
    shortcut_owner=$1
    shortcut_directory=$2
    for application in LanSender LanReceiver; do
        case "$application" in
            LanSender) icon=lansender ;;
            LanReceiver) icon=lanreceiver ;;
        esac
        shortcut=$shortcut_directory/$application.desktop
        [ -x "$shortcut" ]
        [ "$(stat -c '%a' "$shortcut")" = 755 ]
        [ "$(stat -c '%u' "$shortcut")" = "$(id -u "$shortcut_owner")" ]
        [ "$(sed -n 's/^Name=//p' "$shortcut")" = "$application" ]
        [ "$(sed -n 's/^Name\[zh_CN\]=//p' "$shortcut")" = "$application" ]
        [ "$(sed -n 's/^Exec=//p' "$shortcut")" = /usr/bin/$application ]
        [ "$(sed -n 's/^Icon=//p' "$shortcut")" = "$icon" ]
        ! grep -q '^GenericName' "$shortcut"
        desktop-file-validate "$shortcut"
        cmp -s "$shortcut" "/usr/share/applications/$icon.desktop"
    done
}

SUDO_USER=lan-shortcut-sudo dpkg -i "$new_package"
[ "$(dpkg-query -W -f='${Status}' lanbridge)" = 'install ok installed' ]
[ "$(dpkg-query -W -f='${Version}' lanbridge)" = "$(dpkg-deb -f "$new_package" Version)" ]
[ -z "$(dpkg --audit lanbridge)" ]
verify_shortcuts lan-shortcut-sudo "$sudo_home/桌面/应用 快捷方式"
[ ! -e "$gui_home/Desktop/LanSender.desktop" ]
[ ! -e /root/Desktop/LanSender.desktop ] && [ ! -e /root/桌面/LanSender.desktop ]
[ ! -e /tmp/lan-shortcut-config-executed ]
echo 'sudo install: Chinese XDG desktop with spaces, correct names/icons, executable mode and user ownership passed.'

# Reinstall exercises the same maintainer scripts a second time.
SUDO_USER=lan-shortcut-sudo dpkg -i "$new_package"
verify_shortcuts lan-shortcut-sudo "$sudo_home/桌面/应用 快捷方式"
echo 'Repeated install: exactly the same two launchers preserved.'

# A graphical installer has no SUDO_USER and covers existing regular accounts.
unset SUDO_USER
/usr/lib/lanbridge/desktop-shortcuts --install-for-users
verify_shortcuts lan-shortcut-gui "$gui_home/Desktop"
[ ! -e "$disabled_home/LanSender.desktop" ] && [ ! -d "$disabled_home/Desktop" ]
[ ! -e /home/lan-shortcut-system/Desktop/LanSender.desktop ]
[ "$(sed -n 's/^Name=//p' "$custom_home/桌面/LanSender.desktop")" = 'My custom Sender' ]
echo 'Graphical installer fallback: regular accounts only, disabled desktop respected, unrelated custom launcher preserved.'

# A missing/unavailable session metadata backend must not block configuration.
install -d -m 755 -o lan-shortcut-sudo -g lan-shortcut-sudo "$sudo_home/test-bin"
cat > "$sudo_home/test-bin/gio" <<'GIO'
#!/bin/sh
id -u > "$HOME/gio-shortcut-user"
exit 1
GIO
chmod 755 "$sudo_home/test-bin/gio"
runuser -u lan-shortcut-sudo -- env "PATH=$sudo_home/test-bin:$PATH" /usr/lib/lanbridge/desktop-shortcuts --install
[ "$(cat "$sudo_home/gio-shortcut-user")" = "$(id -u lan-shortcut-sudo)" ]
verify_shortcuts lan-shortcut-sudo "$sudo_home/桌面/应用 快捷方式"
echo 'Trust metadata attempted as the user; unavailable metadata backend is nonfatal.'

# User-created symlinks are not followed or replaced.
rm -f "$custom_home/桌面/LanReceiver.desktop"
printf '%s\n' 'Preserved target' > "$custom_home/shortcut-target"
ln -s "$custom_home/shortcut-target" "$custom_home/桌面/LanReceiver.desktop"
/usr/lib/lanbridge/desktop-shortcuts --install-for-users
[ -L "$custom_home/桌面/LanReceiver.desktop" ]
[ "$(cat "$custom_home/shortcut-target")" = 'Preserved target' ]
echo 'User-created symlink and its target preserved.'

printf '%s\n' 'Comment=Personalized shortcut' >> "$gui_home/Desktop/LanSender.desktop"
/usr/lib/lanbridge/desktop-shortcuts --install-for-users
grep -qx 'Comment=Personalized shortcut' "$gui_home/Desktop/LanSender.desktop"
echo 'Customized generated launcher preserved on subsequent install.'

dpkg -r lanbridge
[ ! -e "$sudo_home/桌面/应用 快捷方式/LanSender.desktop" ]
[ ! -e "$sudo_home/桌面/应用 快捷方式/LanReceiver.desktop" ]
[ ! -e "$gui_home/Desktop/LanReceiver.desktop" ]
grep -qx 'Comment=Personalized shortcut' "$gui_home/Desktop/LanSender.desktop"
[ "$(sed -n 's/^Name=//p' "$custom_home/桌面/LanSender.desktop")" = 'My custom Sender' ]
echo 'Uninstall: generated launchers removed, customized and unrelated launchers preserved.'
echo 'Desktop shortcut installation checks passed.'
