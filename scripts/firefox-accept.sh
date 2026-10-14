#!/usr/bin/env bash
# Firefox, end to end: in a throw-away home holding a profile as Firefox
# leaves it, the command applies a theme, a headless Firefox must draw its
# window in the theme's colours, and after undo in its own again.
#
#   scripts/firefox-accept.sh
set -euo pipefail
cd "$(dirname "$0")/.."
if ! command -v firefox >/dev/null 2>&1; then
    echo "NOT RUN: Firefox is not installed"; exit 0
fi
cargo build -q -p velatura-cli
root=$(mktemp -d)
trap 'rm -rf "$root"' EXIT
profile="$root/home/.mozilla/firefox/test.default-release"
mkdir -p "$profile" "$root/home/.local/share" "$root/home/.local/state" "$root/run"
chmod 700 "$root/run"
printf '[Profile0]\nName=default-release\nIsRelative=1\nPath=test.default-release\nDefault=1\n\n[General]\nStartWithLastProfile=1\nVersion=2\n' \
    > "$root/home/.mozilla/firefox/profiles.ini"
# Firefox's own settings: Marionette on a port of its choosing, and a
# blank start, so nothing is fetched from the network.
cat > "$profile/prefs.js" <<'PREFS'
user_pref("marionette.port", 0);
user_pref("browser.startup.page", 0);
user_pref("browser.startup.homepage", "about:blank");
user_pref("browser.newtabpage.enabled", false);
user_pref("browser.shell.checkDefaultBrowser", false);
user_pref("browser.aboutwelcome.enabled", false);
user_pref("datareporting.policy.dataSubmissionEnabled", false);
user_pref("toolkit.telemetry.reportingpolicy.firstRun", false);
user_pref("app.update.disabledForTesting", true);
PREFS
# No session bus: the command needs none for Firefox, and Firefox would
# start the desktop's portals on one.
env -i HOME="$root/home" PATH="$PATH" XDG_RUNTIME_DIR="$root/run" LC_ALL=C.UTF-8 MOZ_HEADLESS=1 \
    GSETTINGS_BACKEND=memory BIN="$PWD/target/debug/velatura" PROFILE="$profile" \
    bash -c '
        set -euo pipefail
        "$BIN" apply ember --only firefox --yes >/dev/null
        cp "$PROFILE/chrome/velatura.css" "$HOME/expected.css"
        python3 -I scripts/firefox_accept.py "$PROFILE" themed "$HOME/expected.css"
        "$BIN" undo --yes >/dev/null
        for f in chrome/velatura.css chrome/userChrome.css user.js; do
            [ ! -e "$PROFILE/$f" ] || { echo "after undo $f is still there"; exit 1; }
        done
        python3 -I scripts/firefox_accept.py "$PROFILE" stock "$HOME/expected.css"'
