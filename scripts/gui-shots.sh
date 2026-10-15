#!/usr/bin/env bash
# Render the app's views to PNG files without touching the desktop: a
# throw-away home folder, a private session bus with its own settings
# database, and a headless compositor.
#
#   scripts/gui-shots.sh <output folder> [WIDTHxHEIGHT] [--rtl] [--applied] [--flow]
#                        [--large-text] [--high-contrast] [--scheduled] [--editor] [--lang es]
#                        [--import <file>] [--template <file>]
#
# --applied applies a theme in the sandbox first, so the views show a
# themed state and a backup. --flow then drives a real apply through the
# confirmation and an undo, and checks the files in the sandbox after each.
# --scheduled installs both programs in the sandbox's ~/.local/bin, turns
# the schedule on against a stand-in systemctl and runs one scheduled
# switch, so the views show a schedule and what it left out.
# --editor makes a new theme in the editor, fixes what is hard to read in
# it, and saves it.
# --import <file> imports a base16, base24 or Kitty file through the app's
# question, choosing another accent than the usual one.
# --template <file> puts a template of the user's in the sandbox first.
# --lang es runs the app in Spanish.
# Needs mutter and dbus-run-session.
set -euo pipefail
cd "$(dirname "$0")/.."
out=$(realpath -m "${1:?output folder}"); shift
size=""; rtl=""; applied=""; flow=""; large=""; contrast=""; scheduled=""; editor=""; lang=""; want_lang=""; import=""; want_import=""; template=""; want_template=""
for a in "$@"; do
    [ -n "$want_lang" ] && { lang=$a; want_lang=""; continue; }
    [ -n "$want_import" ] && { import=$(realpath "$a"); want_import=""; continue; }
    [ -n "$want_template" ] && { template=$(realpath "$a"); want_template=""; continue; }
    case "$a" in --template) want_template=1 ;; --import) want_import=1 ;; --lang) want_lang=1 ;; --rtl) rtl=1 ;; --applied) applied=1 ;; --flow) flow=1 ;; --large-text) large=1 ;; --high-contrast) contrast=1 ;; --scheduled) scheduled=1 ;; --editor) editor=1 ;; *x*) size=$a ;; esac
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
if [ -n "$template" ]; then
    mkdir -p "$root/home/.local/share/velatura/templates"
    cp "$template" "$root/home/.local/share/velatura/templates/"
fi
bin=$PWD/target/debug
if [ -n "$scheduled" ]; then
    mkdir -p "$root/home/.local/bin" "$root/bin"
    cp "$bin/velatura" "$bin/velatura-app" "$root/home/.local/bin/"
    bin=$root/home/.local/bin
    printf '#!/bin/sh\necho "$@" >> "%s"\n' "$root/systemctl.log" > "$root/bin/systemctl"
    chmod +x "$root/bin/systemctl"
    PATH="$root/bin:$PATH"
fi
env -i HOME="$root/home" PATH="$PATH" XDG_RUNTIME_DIR="$root/run" ${lang:+LANG=${lang}_ES.UTF-8 LANGUAGE=$lang} $([ -z "$lang" ] && echo LC_ALL=C.UTF-8) \
    GSETTINGS_BACKEND=dconf VELATURA_SHOTS="$out" VELATURA_SHOTS_SIZE="$size" \
    ${rtl:+VELATURA_SHOTS_RTL=1} ${flow:+VELATURA_SHOTS_FLOW=1} ${editor:+VELATURA_SHOTS_EDITOR=1} ${import:+VELATURA_SHOTS_IMPORT="$import"} APPLIED="$applied" SCHEDULED="$scheduled" LARGE="$large" CONTRAST="$contrast" BIN="$bin" \
    dbus-run-session -- bash -c ' exec 2>/dev/null
        [ -n "$APPLIED" ] && "$BIN/velatura" apply ember --yes >/dev/null
        if [ -n "$SCHEDULED" ]; then
            "$BIN/velatura" switch set day ember >/dev/null
            "$BIN/velatura" switch set night aetherius >/dev/null
            "$BIN/velatura" schedule set >/dev/null
            "$BIN/velatura" switch auto >/dev/null
        fi
        [ -n "$LARGE" ] && gsettings set org.gnome.desktop.interface text-scaling-factor 2.0
        [ -n "$CONTRAST" ] && gsettings set org.gnome.desktop.a11y.interface high-contrast true
        mutter --headless --no-x11 --wayland-display velatura-shots \
            --virtual-monitor 1400x1000 >/dev/null 2>&1 &
        compositor=$!
        for _ in $(seq 50); do [ -S "$XDG_RUNTIME_DIR/velatura-shots" ] && break; read -t 0.1 <> <(:) || true; done
        WAYLAND_DISPLAY=velatura-shots GDK_BACKEND=wayland GTK_A11Y=none timeout 60 "$BIN/velatura-app" 2>/dev/null
        status=$?
        kill $compositor 2>/dev/null
        echo "SANDBOX $("$BIN/velatura" status | head -1)"
        echo "SANDBOX $("$BIN/velatura" schedule | head -1)"
        echo "SANDBOX themes: $("$BIN/velatura" list | tr -s " " | tr "\n" "|")"
        "$BIN/velatura" list | grep "(yours)" | while read -r id _; do
            echo "SANDBOX accent of $id:$("$BIN/velatura" show "$id" | grep -m1 "  accent " | tr -s " ")"
        done
        echo "SANDBOX kitty.conf: $(tr "\n" "|" < "$HOME/.config/kitty/kitty.conf")"
        exit $status'
ls "$out"
