#!/usr/bin/env bash
# Install Velatura from this folder for one user, without root: the
# command and the app into ~/.local/bin, the app's launcher and icons under
# ~/.local/share, and Velatura's Shell extension into the user's extensions
# folder, whatever the prefix. Shipped as install.sh inside each release
# tarball.
#
#   ./install.sh                 install, or update an earlier install
#   ./install.sh --prefix DIR    install under DIR instead of ~/.local
#   ./install.sh --uninstall     remove what this script installed
#
# Removing the program leaves Velatura's changes to the desktop as they
# are: to take those away first, run `velatura restore --original`.
set -euo pipefail
cd "$(dirname "$0")"
prefix=$HOME/.local
uninstall=""
while [ $# -gt 0 ]; do
    case "$1" in
        --prefix) prefix=${2:?--prefix needs a folder}; shift 2 ;;
        --uninstall) uninstall=1; shift ;;
        -h|--help) sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "unknown option: $1" >&2; exit 2 ;;
    esac
done
case "$prefix" in
    /*) ;;
    *) echo "--prefix needs a full path" >&2; exit 2 ;;
esac
# GNOME launches no app whose path has a %, written either way the
# launcher's format allows.
case "$prefix" in
    *[\"\`\$\\\|%]* | *[[:cntrl:]]*) echo "the folder's name has a character a launcher cannot hold" >&2; exit 2 ;;
esac
id=io.github.rafael0rueda.Velatura
files=(
    "bin/velatura"
    "bin/velatura-app"
    "share/applications/$id.desktop"
    "share/icons/hicolor/scalable/apps/$id.svg"
    "share/icons/hicolor/symbolic/apps/$id-symbolic.svg"
)
if [ -n "$uninstall" ]; then
    # The extension first, while the command that installed it is there.
    # Only a copy Velatura installed goes, and not a file changed since.
    if [ -x "$prefix/bin/velatura" ]; then
        "$prefix/bin/velatura" extension remove || true
    fi
    for f in "${files[@]}"; do rm -f "$prefix/$f"; done
    # Velatura's state holds its backups and history, kept with the
    # changes. With nothing there but the empty lock the extension's
    # install took, Velatura never changed this desktop: that goes too.
    state=${XDG_STATE_HOME:-$HOME/.local/state}/velatura
    if [ "$(ls -A "$state" 2>/dev/null)" = lock ] && [ ! -s "$state/lock" ]; then
        rm -f "$state/lock" && rmdir "$state"
    fi
    echo "Removed Velatura from $prefix."
    exit 0
fi
for f in "${files[@]}"; do
    case "$f" in bin/*) mode=755 ;; *) mode=644 ;; esac
    install -D -m "$mode" "$f" "$prefix/$f"
done
# The launcher names the app by its full path: the folder need not be on
# the session's PATH. sed reads & as what it matched, so it is escaped.
exec_path=${prefix//&/\\&}
sed -i "s|^Exec=velatura-app\$|Exec=\"$exec_path/bin/velatura-app\"|" \
    "$prefix/share/applications/$id.desktop"
echo "Installed $("$prefix/bin/velatura" --version) in $prefix."
# The parts Velatura's own extension shows need the copy of it this
# version brings: installed, or brought up to date. It does nothing if the
# copy is already this one, and leaves alone a later one or someone else's.
"$prefix/bin/velatura" extension install ||
    echo "Note: Velatura's Shell extension was left as it is."
case ":$PATH:" in
    *":$prefix/bin:"*) ;;
    *) echo "Note: $prefix/bin is not on PATH, so the command is found only by its full path." ;;
esac
