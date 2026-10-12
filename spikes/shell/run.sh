#!/usr/bin/env bash
# Phase 0 spike: User Themes behaviour in a throw-away headless GNOME Shell.
# Fully isolated from the real session: private D-Bus, private dconf, private
# XDG dirs. The real User Themes extension is only read (symlinked in).
#
#   spikes/shell/run.sh [outdir]
set -euo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
OUT=$(realpath -m "${1:-/tmp/velatura-shell-spike}")
UT=user-theme@gnome-shell-extensions.gcampax.github.com
UT_DIR=${VEL_USER_THEME_DIR:-$HOME/.local/share/gnome-shell/extensions/$UT}
[ -d "$UT_DIR" ] || UT_DIR=/usr/share/gnome-shell/extensions/$UT
[ -d "$UT_DIR" ] || { echo "User Themes extension not found"; exit 2; }

rm -rf "$OUT" && mkdir -p "$OUT"
T=$(mktemp -d /tmp/velatura-env.XXXXXX)
mkdir -p "$T"/{config,data,cache,state,run,home} "$T/data/gnome-shell/extensions"
chmod 700 "$T/run"
ext="$T/data/gnome-shell/extensions"
ln -s "$UT_DIR" "$ext/$UT"
ln -s "$HERE/velatura-probe@test" "$ext/"

themes="$T/data/themes"
for n in a b bad; do mkdir -p "$themes/Velatura-$n/gnome-shell"; done
echo '#panel { background-color: #aa0000; }' > "$themes/Velatura-a/gnome-shell/gnome-shell.css"
echo '#panel { background-color: #0000aa; }' > "$themes/Velatura-b/gnome-shell/gnome-shell.css"
printf '#panel { background-color: ; } }}} @@@ \n#panel { color: 12px }\n#panel { background-color: #123456; }\n' \
    > "$themes/Velatura-bad/gnome-shell/gnome-shell.css"

export HOME=$T/home
export XDG_CONFIG_HOME=$T/config XDG_DATA_HOME=$T/data XDG_CACHE_HOME=$T/cache
export XDG_STATE_HOME=$T/state XDG_RUNTIME_DIR=$T/run
export VEL_OUT=$OUT VEL_THEMES=$themes VEL_USER_THEME_SCHEMAS=$UT_DIR/schemas

dbus-run-session -- bash -c '
    set -e
    gsettings set org.gnome.shell enabled-extensions "['"'$UT'"', '"'velatura-probe@test'"']"
    gsettings set org.gnome.shell disable-user-extensions false
    gsettings set org.gnome.shell disable-extension-version-validation true
    gsettings set org.gnome.desktop.interface color-scheme prefer-dark
    gnome-shell --headless --no-x11 --virtual-monitor 1280x720 > "$VEL_OUT/shell.log" 2>&1 &
    SHELL_PID=$!
    for _ in $(seq 120); do
        [ -e "$VEL_OUT/done" ] || [ -e "$VEL_OUT/error.txt" ] && break
        kill -0 $SHELL_PID 2>/dev/null || break
        sleep 0.5
    done
    kill $SHELL_PID 2>/dev/null || true
    wait $SHELL_PID 2>/dev/null || true
'
mountpoint -q "$T/run/doc" && fusermount3 -u "$T/run/doc"
rm -rf "$T"
gnome-shell --version > "$OUT/version.txt"
[ -e "$OUT/error.txt" ] && { cat "$OUT/error.txt"; exit 1; }
[ -e "$OUT/done" ] || { echo "no result; see $OUT/shell.log"; tail -20 "$OUT/shell.log"; exit 1; }
cat "$OUT/results.json"
grep -i -E "user theme|css|stylesheet" "$OUT/shell.log" | head -20 || true
