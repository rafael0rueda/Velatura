#!/usr/bin/env bash
# Shell acceptance: the velatura command applies a theme in a throw-away
# home, a headless GNOME Shell with the User Themes extension starts there,
# and the panel's computed colours, and the accent of a checked quick
# settings toggle, are compared with the theme's. A second
# theme is then applied while the Shell runs, to check that it reloads.
# Last, through Velatura's own extension, the Top Bar Style part draws
# the bar as pills and then as a full bar, and is taken away; and the UPS
# indicator follows a stand-in for NUT's server on its usual port, which
# is skipped if something already listens there.
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
ln -s "$PWD/scripts/shell-accept/velatura-rival@test" "$ext/"

env -i HOME="$root/home" PATH="$PATH" XDG_RUNTIME_DIR="$root/run" LC_ALL=C.UTF-8 \
    GSETTINGS_BACKEND=dconf VEL_OUT="$root/out" BIN="$PWD/target/debug/velatura" UT="$ut" \
    FAKE_UPSD="$PWD/scripts/shell-accept/fake-upsd.py" \
    dbus-run-session -- bash -c '
    exec 2>/dev/null
    panel() { cat "$VEL_OUT/panel.txt" 2>/dev/null || true; }
    top_bar() { cat "$VEL_OUT/top-bar.txt" 2>/dev/null || true; }
    ups() { cat "$VEL_OUT/ups.txt" 2>/dev/null || true; }
    wait_ups() { # shown, watts, state, colour of the watts, label
        for _ in $(seq 60); do
            [ "$(ups)" = "$1" ] && { echo "ok: $2: UPS is $1"; return 0; }
            read -t 0.5 <> <(:) || true
        done
        echo "FAILED: $2: expected $1, UPS is $(ups)"; return 1
    }
    wait_bar() { # panel, button and clock fills, label
        for _ in $(seq 60); do
            [ "$(top_bar)" = "$1" ] && { echo "ok: $2: top bar is $1"; return 0; }
            read -t 0.5 <> <(:) || true
        done
        echo "FAILED: $2: expected $1, top bar is $(top_bar)"; return 1
    }
    wait_hidden() { # separator background, clock button fill, label
        for _ in $(seq 60); do
            [ "$(cat "$VEL_OUT/hidden.txt" 2>/dev/null)" = "$1" ] && { echo "ok: $2: $1"; return 0; }
            read -t 0.5 <> <(:) || true
        done
        echo "FAILED: $2: expected $1, got $(cat "$VEL_OUT/hidden.txt" 2>/dev/null)"; return 1
    }
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
    # Out of the overview, where the Shell'"'"'s own bar is transparent, as
    # the theme leaves it.
    VEL_NO_OVERVIEW=1 gnome-shell --headless --no-x11 --virtual-monitor 1280x720 >"$VEL_OUT/shell.log" 2>&1 &
    shell=$!
    status=0
    wait_for "#110d0f #dbc7b0 #e0914f" "Ember, applied before the Shell started" || status=1
    wait_hidden "#00000000 none" "through User Themes, what the Shell hides stays hidden" || status=1
    "$BIN" apply aetherius --only gnome:shell --yes >/dev/null || status=1
    wait_for "#0d101a #d3d9ee #8493d8" "Aetherius, applied while the Shell runs" || status=1
    "$BIN" undo >/dev/null && "$BIN" undo >/dev/null || status=1
    wait_for "#000000 #ffffff #3584e4" "stock colours after undoing both" || status=1
    kill $shell 2>/dev/null; wait $shell 2>/dev/null
    grep -i -E "velatura.*(error|warn)|error.*gnome-shell\.css" "$VEL_OUT/shell.log" && status=1

    # Second round: another extension colours the panel. Through User Themes
    # the theme loses to it; through Velatura'"'"'s own extension it wins.
    rm -f "$VEL_OUT/panel.txt" "$VEL_OUT/hidden.txt"
    gsettings set org.gnome.shell enabled-extensions "['"'"'$UT'"'"', '"'"'velatura-accept@test'"'"', '"'"'velatura-rival@test'"'"']"
    "$BIN" apply ember --only gnome:shell --yes >/dev/null || status=1
    gnome-shell --headless --no-x11 --virtual-monitor 1280x720 >"$VEL_OUT/shell2.log" 2>&1 &
    shell=$!
    wait_for "#123456 #fedcba #e0914f" "User Themes loses to another extension" || status=1
    kill $shell 2>/dev/null; wait $shell 2>/dev/null

    rm -f "$VEL_OUT/panel.txt" "$VEL_OUT/hidden.txt"
    "$BIN" extension install >/dev/null || status=1
    # Turning it on is the user'"'"'s to do; here the script is the user.
    gsettings set org.gnome.shell enabled-extensions "['"'"'velatura@rafael0rueda.github.io'"'"', '"'"'$UT'"'"', '"'"'velatura-accept@test'"'"', '"'"'velatura-rival@test'"'"']"
    "$BIN" apply ember --only gnome:shell --yes >/dev/null || status=1
    VEL_NO_OVERVIEW=1 gnome-shell --headless --no-x11 --virtual-monitor 1280x720 >"$VEL_OUT/shell3.log" 2>&1 &
    shell=$!
    wait_for "#110d0f #dbc7b0 #e0914f" "Ember through Velatura'"'"'s extension, over the other one" || status=1
    wait_hidden "#00000000 none" "through Velatura'"'"'s extension, what the Shell hides stays hidden" || status=1
    "$BIN" apply aetherius --only gnome:shell --yes >/dev/null || status=1
    wait_for "#0d101a #d3d9ee #8493d8" "Aetherius, the file replaced while the Shell runs" || status=1
    "$BIN" undo >/dev/null && "$BIN" undo >/dev/null || status=1
    wait_for "#123456 #fedcba #3584e4" "the other extension again after undoing both" || status=1
    kill $shell 2>/dev/null; wait $shell 2>/dev/null
    grep -i -E "velatura.*(error|warn|critical)" "$VEL_OUT/shell3.log" && status=1

    # The top bar: pills at 60 %, then a full bar at 80 %, both in
    # Aetherius (surface #1e2438, bar #0d101a), then taken away.
    rm -f "$VEL_OUT/panel.txt" "$VEL_OUT/top-bar.txt"
    gsettings set org.gnome.shell enabled-extensions "['"'"'velatura@rafael0rueda.github.io'"'"', '"'"'velatura-accept@test'"'"']"
    both="--only gnome:shell --only gnome:top-bar"
    "$BIN" apply aetherius $both --yes >/dev/null || status=1
    VEL_NO_OVERVIEW=1 gnome-shell --headless --no-x11 --virtual-monitor 1280x720 >"$VEL_OUT/shell4.log" 2>&1 &
    shell=$!
    wait_bar "#00000000 #1e243899 #1e243899" "pills at 60 %, the clock'"'"'s label too" || status=1
    "$BIN" top-bar style bar >/dev/null && "$BIN" top-bar opacity 80 >/dev/null || status=1
    "$BIN" apply aetherius $both --yes >/dev/null || status=1
    wait_bar "#0d101acc #00000000 none" "a full bar at 80 %, the buttons left alone" || status=1
    # Not turned on in this home, so an apply by hand takes it away.
    "$BIN" apply aetherius --yes >/dev/null || status=1
    wait_bar "#0d101aff #00000000 none" "the bar as the Shell part leaves it, once taken away" || status=1
    kill $shell 2>/dev/null; wait $shell 2>/dev/null
    grep -i -E "velatura.*(error|warn|critical)" "$VEL_OUT/shell4.log" && status=1

    # The UPS, in Aetherius (text #d3d9ee, warning #eba868, error #e5625e).
    if python3 -I -c "import socket; socket.create_server((\"127.0.0.1\", 3493)).close()"; then
        echo "OL CHRG" > "$VEL_OUT/ups-status"
        python3 -I "$FAKE_UPSD" 3493 "$VEL_OUT/ups-status" &
        upsd=$!
        "$BIN" apply aetherius --only gnome:ups --yes >/dev/null || status=1
        VEL_NO_OVERVIEW=1 gnome-shell --headless --no-x11 --virtual-monitor 1280x720 >"$VEL_OUT/shell5.log" 2>&1 &
        shell=$!
        wait_ups "shown 245W normal #d3d9ee" "the watts NUT reports, on AC power" || status=1
        echo "OB DISCHRG" > "$VEL_OUT/ups-status"
        wait_ups "shown 245W on-battery #eba868" "on battery, in the warning colour" || status=1
        echo "OB LB" > "$VEL_OUT/ups-status"
        wait_ups "shown 245W low-battery #e5625e" "battery low, in the error colour" || status=1
        kill $upsd 2>/dev/null; wait $upsd 2>/dev/null
        wait_ups "hidden 245W low-battery #e5625e" "hidden once NUT stops answering" || status=1
        "$BIN" apply aetherius --yes >/dev/null || status=1
        wait_ups "none" "gone once the part is taken away" || status=1
        kill $shell 2>/dev/null; wait $shell 2>/dev/null
        grep -i -E "velatura.*(error|warn|critical)|JS ERROR" "$VEL_OUT/shell5.log" && status=1
    else
        echo "NOT RUN: the UPS round, as something listens on port 3493"
    fi
    exit $status' 2>/dev/null
