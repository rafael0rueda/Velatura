#!/usr/bin/env bash
# Shell acceptance: the velatura command applies a theme in a throw-away
# home, a headless GNOME Shell with the User Themes extension starts there,
# and the panel's computed colours are compared with the theme's. A second
# theme is then applied while the Shell runs, to check that it reloads.
#
# Nothing of the real session is touched: private session bus, private
# settings database, private folders. The installed User Themes extension is
# only read, through a link. Needs gnome-shell, dbus-run-session and User
# Themes.
#
#   scripts/shell-accept.sh
set -euo pipefail
cd "$(dirname "$0")/.."
ut=user-theme@gnome-shell-extensions.gcampax.github.com
ut_dir=$HOME/.local/share/gnome-shell/extensions/$ut
[ -d "$ut_dir" ] || ut_dir=/usr/share/gnome-shell/extensions/$ut
[ -d "$ut_dir" ] || { echo "NOT RUN: the User Themes extension is not installed"; exit 0; }
cargo build -q -p velatura-cli
root=$(mktemp -d)
trap 'for m in "$root"/run/doc "$root"/run/gvfs; do fusermount3 -u "$m" 2>/dev/null || true; done; rm -rf "$root"' EXIT
ext=$root/home/.local/share/gnome-shell/extensions
mkdir -p "$ext" "$root/home/.config" "$root/home/.local/state" "$root/run" "$root/out"
chmod 700 "$root/run"
ln -s "$ut_dir" "$ext/$ut"
ln -s "$PWD/scripts/shell-accept/velatura-accept@test" "$ext/"

env -i HOME="$root/home" PATH="$PATH" XDG_RUNTIME_DIR="$root/run" LC_ALL=C.UTF-8 \
    GSETTINGS_BACKEND=dconf VEL_OUT="$root/out" BIN="$PWD/target/debug/velatura" UT="$ut" \
    dbus-run-session -- bash -c '
    exec 2>/dev/null
    panel() { cat "$VEL_OUT/panel.txt" 2>/dev/null || true; }
    wait_for() { # colours, label
        for _ in $(seq 60); do
            [ "$(panel)" = "$1" ] && { echo "ok: $2: panel is $1"; return 0; }
            read -t 0.5 <> <(:) || true
        done
        echo "FAILED: $2: expected $1, panel is $(panel)"; return 1
    }
    gsettings set org.gnome.shell enabled-extensions "['"'"'$UT'"'"', '"'"'velatura-accept@test'"'"']"
    gsettings set org.gnome.shell disable-user-extensions false
    gsettings set org.gnome.shell disable-extension-version-validation true
    "$BIN" apply ember --only gnome:shell --yes >/dev/null || { echo "FAILED: apply ember"; exit 1; }
    gnome-shell --headless --no-x11 --virtual-monitor 1280x720 >"$VEL_OUT/shell.log" 2>&1 &
    shell=$!
    status=0
    wait_for "#110d0f #dbc7b0" "Ember, applied before the Shell started" || status=1
    "$BIN" apply aetherius --only gnome:shell --yes >/dev/null || status=1
    wait_for "#0d101a #d3d9ee" "Aetherius, applied while the Shell runs" || status=1
    "$BIN" undo >/dev/null && "$BIN" undo >/dev/null || status=1
    wait_for "#000000 #ffffff" "stock colours after undoing both" || status=1
    kill $shell 2>/dev/null; wait $shell 2>/dev/null
    grep -i -E "velatura.*(error|warn)|error.*gnome-shell\.css" "$VEL_OUT/shell.log" && status=1
    exit $status' 2>/dev/null
