#!/usr/bin/env bash
# The single test gate. Nothing is committed, merged or pushed unless this
# passes. GitHub Actions runs exactly this script.
#
#   scripts/check.sh
#
# VELATURA_CI=1 makes a missing tool an error instead of a skipped step.
set -euo pipefail
cd "$(dirname "$0")/.."

step() { printf '\n== %s\n' "$*"; }
skipped=()

step "format"
if cargo fmt --version >/dev/null 2>&1; then
    cargo fmt --all --check
elif [ -n "${VELATURA_CI:-}" ]; then
    echo "rustfmt is required in CI"; exit 1
else
    echo "SKIPPED: rustfmt is not installed (sudo dnf install rustfmt). CI still enforces it."
    skipped+=(format)
fi

step "lint"
cargo clippy --workspace --all-targets --locked -- -D warnings

step "test"
cargo test --workspace --locked

step "dependencies"
# A cap on third-party crates per binary, so growth is a decision.
cap=45
count=$(cargo tree -p velatura-cli -e normal --prefix none --locked | sed 's/ (.*//' | sort -u | grep -vc '^velatura-')
echo "third-party crates in the velatura binary: $count (cap $cap)"
[ "$count" -le "$cap" ] || { echo "crate count is over the cap"; exit 1; }

python3 scripts/check_licenses.py

step "generated files"
# The apps themselves must accept what the renderers produce.
golden=crates/velatura-theme/tests/golden
for major in 3 4; do
    if python3 -c "import gi; gi.require_version('Gtk', '$major.0'); from gi.repository import Gtk" 2>/dev/null; then
        python3 scripts/gtk_accept.py "$major" "$golden"/*.gtk.css
    elif [ -n "${VELATURA_CI:-}" ]; then
        echo "GTK $major and PyGObject are required in CI"; exit 1
    else
        echo "SKIPPED: GTK $major is not available to Python"; skipped+=("gtk$major")
    fi
done
# And a libadwaita app must compute those colours, over earlier definitions.
# Needs a display: the session's, or a headless compositor started for it.
probe() {
    for f in "$golden"/*.gtk.css; do python3 scripts/gtk-probe.py "$f"; done
    # Informational: has libadwaita's own palette moved since the table the
    # app uses to keep its window in stock colours was generated?
    python3 scripts/gen-stock-colors.py --check crates/velatura-gui/src/stock.rs || true
}
if [ -n "${WAYLAND_DISPLAY:-}${DISPLAY:-}" ]; then
    probe
elif command -v mutter >/dev/null 2>&1 && command -v dbus-run-session >/dev/null 2>&1; then
    export -f probe; export golden
    run=$(mktemp -d); chmod 700 "$run"
    XDG_RUNTIME_DIR="$run" dbus-run-session -- bash -c '
        mutter --headless --no-x11 --wayland-display velatura-probe --virtual-monitor 800x600 >/dev/null 2>&1 &
        compositor=$!
        for _ in $(seq 50); do [ -S "$XDG_RUNTIME_DIR/velatura-probe" ] && break; read -t 0.1 <> <(:) || true; done
        WAYLAND_DISPLAY=velatura-probe GDK_BACKEND=wayland probe; status=$?
        kill $compositor 2>/dev/null; exit $status' 2>/dev/null
    rm -rf "$run"
else
    echo "NOT RUN: the colour probe needs a display or a headless compositor"
fi
if command -v ghostty >/dev/null 2>&1; then
    for f in "$golden"/*.ghostty.conf; do
        ghostty +validate-config --config-file="$f" && echo "Ghostty accepts $f"
    done
else
    echo "NOT RUN: Ghostty is not installed (it is not packaged for the CI image)"
fi

step "app"
# The window, driven for real in a throw-away session: first apply through
# the confirmation, then undo, with the sandbox's files checked after.
desktop-file-validate data/io.github.rafael0rueda.Velatura.desktop
before=$(mktemp)
cp po/velatura.pot "$before"
scripts/update-pot.sh >/dev/null
if ! cmp -s "$before" po/velatura.pot; then
    rm -f "$before"
    echo "po/velatura.pot was out of date and has been regenerated: commit it"; exit 1
fi
rm -f "$before"
if command -v mutter >/dev/null 2>&1 && command -v dbus-run-session >/dev/null 2>&1; then
    shots=$(mktemp -d)
    flow=$(scripts/gui-shots.sh "$shots" --flow 2>/dev/null | grep -E '^(FLOW|SANDBOX)' || true)
    rm -rf "$shots"
    echo "$flow"
    for want in 'FLOW confirmation shown' 'FLOW applied=Some("ember/default")' \
        'FLOW undone applied=None' 'SANDBOX No theme applied.' 'SANDBOX kitty.conf: font_size 12|'; do
        grep -qxF "$want" <<<"$flow" || { echo "app flow check failed, missing: $want"; exit 1; }
    done
    # With a theme applied to GTK apps, the app's own window keeps
    # libadwaita's colours: not Ember's background.
    shots=$(mktemp -d)
    own=$(scripts/gui-shots.sh "$shots" --applied 2>/dev/null | grep '^SHOTS own' || true)
    rm -rf "$shots"
    echo "$own"
    case "$own" in
        "SHOTS own window_bg_color=#191316" | "") echo "the app's window took the theme's colours"; exit 1 ;;
    esac
else
    echo "NOT RUN: the app flow check needs mutter (not in the CI image)"
fi

step "shell"
# The command applies themes in a sandbox and a headless GNOME Shell must
# show their colours on the panel, reload on a new theme, and return to
# stock after undo.
if ! command -v gnome-shell >/dev/null 2>&1 || ! command -v dbus-run-session >/dev/null 2>&1; then
    echo "NOT RUN: the Shell check needs gnome-shell (not in the CI image)"
elif [ ! -S /run/dbus/system_bus_socket ]; then
    # The Shell asks the login manager for its session when it starts.
    echo "NOT RUN: the Shell check needs a system bus, which a container does not have"
else
    scripts/shell-accept.sh
fi

step "docs"
python3 scripts/check_docs.py

if [ ${#skipped[@]} -gt 0 ]; then
    printf '\nPASSED, with skipped steps: %s\n' "${skipped[*]}"
else
    printf '\nPASSED\n'
fi
