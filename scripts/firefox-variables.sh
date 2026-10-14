#!/usr/bin/env bash
# Write the list of CSS variables a Firefox version's own window
# stylesheets read, taken from its installed files. The lists are
# fixtures: a test checks that every variable Velatura sets is in each.
#
#   scripts/firefox-variables.sh [firefox folder]   -> writes
#       crates/velatura-theme/tests/fixtures/firefox-variables-<major>.txt
#
# The folder is the one holding omni.ja, /usr/lib64/firefox by default.
set -euo pipefail
cd "$(dirname "$0")/.."
dir=${1:-/usr/lib64/firefox}
[ -f "$dir/omni.ja" ] || dir=/usr/lib/firefox
[ -f "$dir/omni.ja" ] || { echo "no omni.ja in $dir"; exit 1; }
version=$(sed -n 's/^Version=//p' "$dir/application.ini" | head -1)
major=${version%%.*}
out=crates/velatura-theme/tests/fixtures/firefox-variables-$major.txt
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
# Firefox's jars put their index first, which Python's zip reader refuses.
for jar in "$dir/omni.ja" "$dir/browser/omni.ja"; do
    mkdir -p "$tmp/$(basename "$(dirname "$jar")")"
    unzip -qo "$jar" '*.css' -x '*/devtools/*' -d "$tmp/$(basename "$(dirname "$jar")")" 2>/dev/null || true
done
grep -rhoE 'var\(\s*--[A-Za-z0-9_-]+' "$tmp" | sed 's/^var(\s*//' | sort -u > "$out"
echo "Firefox $version: $(wc -l < "$out") variables read, in $out"
