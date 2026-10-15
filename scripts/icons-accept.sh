#!/usr/bin/env bash
# Recolored icons, end to end: in a throw-away home with a private session,
# the command recolors the system's Adwaita, GTK is asked where it finds
# each icon, and undo puts the setting and the folder back.
#
#   scripts/icons-accept.sh
set -euo pipefail
cd "$(dirname "$0")/.."
if [ ! -f /usr/share/icons/Adwaita/index.theme ]; then
    echo "NOT RUN: the Adwaita icon theme is not installed"; exit 0
fi
cargo build -q -p velatura-cli
root=$(mktemp -d)
trap 'rm -rf "$root"' EXIT
mkdir -p "$root/home/.local/share" "$root/home/.local/state" "$root/run"
chmod 700 "$root/run"
env -i HOME="$root/home" PATH="$PATH" XDG_RUNTIME_DIR="$root/run" LC_ALL=C.UTF-8 GIO_USE_VFS=local \
    GSETTINGS_BACKEND=dconf BIN="$PWD/target/debug/velatura" ROOT="$root" \
    dbus-run-session -- bash -c '
        set -euo pipefail
        gsettings set org.gnome.desktop.interface icon-theme Adwaita
        "$BIN" apply ember --only icons:recolor --yes >/dev/null
        name=$(gsettings get org.gnome.desktop.interface icon-theme | tr -d "'\''")
        case "$name" in Velatura-adwaita-*) ;; *) echo "the icon theme is $name, not a set"; exit 1 ;; esac
        accent=$("$BIN" show ember | sed -n "s/^  accent  *\(#[0-9a-f]\{6\}\).*/\1/p" | head -1)
        python3 scripts/icons_accept.py "$HOME/.local/share/icons" "$name" Adwaita "$accent"
        "$BIN" undo --yes >/dev/null
        now=$(gsettings get org.gnome.desktop.interface icon-theme)
        [ "$now" = "'\''Adwaita'\''" ] || { echo "after undo the icon theme is $now"; exit 1; }
        [ ! -e "$HOME/.local/share/icons/$name" ] || { echo "after undo the set is still there"; exit 1; }
        echo "undo puts back Adwaita and takes the set away"'

# The same for an icon theme made of pictures, and for a cursor theme:
# small fixtures, read back by readers that are not Velatura's.
fixtures=crates/velatura-engine/tests/fixtures/icons
root2=$(mktemp -d)
trap 'rm -rf "$root" "$root2"' EXIT
mkdir -p "$root2/home/.local/share/icons" "$root2/home/.local/state" "$root2/run"
chmod 700 "$root2/run"
cp -r "$fixtures/Frosty" "$fixtures/Pointy" "$root2/home/.local/share/icons/"
env -i HOME="$root2/home" PATH="$PATH" XDG_RUNTIME_DIR="$root2/run" LC_ALL=C.UTF-8 GIO_USE_VFS=local \
    GSETTINGS_BACKEND=dconf BIN="$PWD/target/debug/velatura" \
    dbus-run-session -- bash -c '
        set -euo pipefail
        gsettings set org.gnome.desktop.interface icon-theme Frosty
        gsettings set org.gnome.desktop.interface cursor-theme Pointy
        "$BIN" apply ember --only icons:recolor --only cursors:recolor --yes >/dev/null
        icons=$(gsettings get org.gnome.desktop.interface icon-theme | tr -d "'\''")
        cursors=$(gsettings get org.gnome.desktop.interface cursor-theme | tr -d "'\''")
        color() { "$BIN" show ember | sed -n "s/^  $1  *\(#[0-9a-f]\{6\}\).*/\1/p" | head -1; }
        python3 -I scripts/pictures_accept.py icons "$HOME/.local/share/icons/$icons" "$(color accent)"
        python3 -I scripts/pictures_accept.py cursors "$HOME/.local/share/icons/$cursors" "$(color bg)" "$(color fg)"
        "$BIN" undo --yes >/dev/null
        [ "$(gsettings get org.gnome.desktop.interface cursor-theme)" = "'\''Pointy'\''" ] || { echo "after undo the cursor is not Pointy"; exit 1; }
        echo "undo puts back Frosty and Pointy"'
