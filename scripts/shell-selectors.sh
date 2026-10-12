#!/usr/bin/env bash
# Write the list of selectors in a GNOME Shell version's own dark
# stylesheet, taken from that version's package in a Fedora container. The
# lists are fixtures: a test checks that every selector Velatura overrides
# exists in each of them.
#
#   scripts/shell-selectors.sh fedora:45     -> prints "51" and writes
#       crates/velatura-theme/tests/fixtures/shell-selectors-51.txt
set -euo pipefail
cd "$(dirname "$0")/.."
image=${1:?a Fedora image, for example fedora:45}
out=crates/velatura-theme/tests/fixtures
podman run --rm -i "$image" bash -ec '
    dnf install -y -q --setopt=install_weak_deps=False gnome-shell glib2-devel python3 >/dev/null 2>&1
    gnome-shell --version | grep -o "[0-9]*" | head -1
    gresource extract /usr/share/gnome-shell/gnome-shell-theme.gresource \
        /org/gnome/shell/theme/gnome-shell-dark.css > /tmp/dark.css
    python3 -c "
import re
s = re.sub(r\"/\*.*?\*/\", \"\", open(\"/tmp/dark.css\").read(), flags=re.S)
found = set()
for selector, _ in re.findall(r\"([^{}]+)\{([^{}]*)\}\", s):
    for part in selector.split(\",\"):
        part = \" \".join(part.split())
        if part and not part.startswith(\"@\"):
            found.add(part)
print(\"\n\".join(sorted(found)))
"' > /tmp/selectors.$$
version=$(head -1 /tmp/selectors.$$)
tail -n +2 /tmp/selectors.$$ > "$out/shell-selectors-$version.txt"
rm -f /tmp/selectors.$$
echo "GNOME Shell $version: $(wc -l < "$out/shell-selectors-$version.txt") selectors"
