#!/usr/bin/env bash
# Render the app's views to PNG files without touching the desktop: a
# throw-away home folder, a private session bus with its own settings
# database, and a headless compositor.
#
#   scripts/gui-shots.sh <output folder> [WIDTHxHEIGHT] [--rtl] [--applied] [--flow]
#
# --applied applies a theme in the sandbox first, so the views show a
# themed state and a backup. --flow then drives a real apply through the
# confirmation and an undo, and checks the files in the sandbox after each. Needs mutter and dbus-run-session.
set -euo pipefail
cd "$(dirname "$0")/.."
out=$(realpath -m "${1:?output folder}"); shift
size=""; rtl=""; applied=""; flow=""
for a in "$@"; do
    case "$a" in --rtl) rtl=1 ;; --applied) applied=1 ;; --flow) flow=1 ;; *x*) size=$a ;; esac
done
cargo build -q -p velatura-gui -p velatura-cli
root=$(mktemp -d)
# The private session may mount helper file systems in the runtime folder.
trap 'for m in "$root"/run/doc "$root"/run/gvfs; do fusermount3 -u "$m" 2>/dev/null || true; done; rm -rf "$root"' EXIT
mkdir -p "$out" "$root/home/.config/kitty" "$root/home/.config/ghostty" \
    "$root/home/.config/gtk-4.0" "$root/home/.local/share" "$root/home/.local/state" "$root/run"
chmod 700 "$root/run"
printf 'font_size 12\n' > "$root/home/.config/kitty/kitty.conf"
printf 'font-size = 12\n' > "$root/home/.config/ghostty/config"
printf '/* mine */\n' > "$root/home/.config/gtk-4.0/gtk.css"
bin=$PWD/target/debug
env -i HOME="$root/home" PATH="$PATH" XDG_RUNTIME_DIR="$root/run" LC_ALL=C.UTF-8 \
    GSETTINGS_BACKEND=dconf VELATURA_SHOTS="$out" VELATURA_SHOTS_SIZE="$size" \
    ${rtl:+VELATURA_SHOTS_RTL=1} ${flow:+VELATURA_SHOTS_FLOW=1} APPLIED="$applied" BIN="$bin" \
    dbus-run-session -- bash -c ' exec 2>/dev/null
        [ -n "$APPLIED" ] && "$BIN/velatura" apply ember --yes >/dev/null
        mutter --headless --no-x11 --wayland-display velatura-shots \
            --virtual-monitor 1400x1000 >/dev/null 2>&1 &
        compositor=$!
        for _ in $(seq 50); do [ -S "$XDG_RUNTIME_DIR/velatura-shots" ] && break; read -t 0.1 <> <(:) || true; done
        WAYLAND_DISPLAY=velatura-shots GDK_BACKEND=wayland GTK_A11Y=none timeout 60 "$BIN/velatura-app" 2>/dev/null
        status=$?
        kill $compositor 2>/dev/null
        echo "SANDBOX $("$BIN/velatura" status | head -1)"
        echo "SANDBOX kitty.conf: $(tr "\n" "|" < "$HOME/.config/kitty/kitty.conf")"
        exit $status'
ls "$out"
