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
scripts/update-pot.sh >/dev/null
git diff --quiet -- po/velatura.pot || { echo "po/velatura.pot is out of date: run scripts/update-pot.sh"; exit 1; }
if command -v mutter >/dev/null 2>&1 && command -v dbus-run-session >/dev/null 2>&1; then
    shots=$(mktemp -d)
    flow=$(scripts/gui-shots.sh "$shots" --flow 2>/dev/null | grep -E '^(FLOW|SANDBOX)' || true)
    rm -rf "$shots"
    echo "$flow"
    for want in 'FLOW confirmation shown' 'FLOW applied=Some("ember/default")' \
        'FLOW undone applied=None' 'SANDBOX No theme applied.' 'SANDBOX kitty.conf: font_size 12|'; do
        grep -qxF "$want" <<<"$flow" || { echo "app flow check failed, missing: $want"; exit 1; }
    done
else
    echo "NOT RUN: the app flow check needs mutter (not in the CI image)"
fi

step "docs"
python3 scripts/check_docs.py

if [ ${#skipped[@]} -gt 0 ]; then
    printf '\nPASSED, with skipped steps: %s\n' "${skipped[*]}"
else
    printf '\nPASSED\n'
fi
