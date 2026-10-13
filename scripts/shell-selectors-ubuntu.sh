#!/usr/bin/env bash
# Write the list of selectors in the Shell stylesheet Ubuntu's own session
# uses (Yaru), taken from that release's packages in an Ubuntu container.
# Only selectors present in both the light and the dark stylesheet are
# listed, since either may be in use. The lists are fixtures: a test checks
# that every selector Velatura overrides exists in each of them.
#
#   scripts/shell-selectors-ubuntu.sh ubuntu:25.10   -> prints "49" and writes
#       crates/velatura-theme/tests/fixtures/shell-selectors-ubuntu-49.txt
set -euo pipefail
cd "$(dirname "$0")/.."
image=${1:?an Ubuntu image, for example ubuntu:25.10}
out=crates/velatura-theme/tests/fixtures
podman run --rm -i "$image" bash -ec '
    export DEBIAN_FRONTEND=noninteractive
    # A release past its end of life has its packages moved aside.
    apt-get update -qq >/dev/null 2>&1 || {
        sed -i "s|//archive.ubuntu.com|//old-releases.ubuntu.com|; s|//security.ubuntu.com|//old-releases.ubuntu.com|" \
            /etc/apt/sources.list /etc/apt/sources.list.d/* 2>/dev/null
        apt-get update -qq >/dev/null 2>&1
    }
    apt-get install -y -qq --no-install-recommends yaru-theme-gnome-shell gnome-shell-common libglib2.0-bin python3 >/dev/null 2>&1
    dpkg-query -W -f "\${Version}\n" gnome-shell-common | grep -o "^[0-9]*"
    theme=/usr/share/gnome-shell/theme/Yaru/gnome-shell-theme.gresource
    gresource extract $theme /org/gnome/shell/theme/Yaru/gnome-shell.css > /tmp/light.css
    gresource extract $theme /org/gnome/shell/theme/Yaru/gnome-shell-dark.css > /tmp/dark.css
    python3 -c "
import re
def selectors(path):
    s = re.sub(r\"/\*.*?\*/\", \"\", open(path).read(), flags=re.S)
    found = set()
    for selector, _ in re.findall(r\"([^{}]+)\{([^{}]*)\}\", s):
        for part in selector.split(\",\"):
            part = \" \".join(part.split())
            if part and not part.startswith(\"@\"):
                found.add(part)
    return found
print(\"\n\".join(sorted(selectors(\"/tmp/light.css\") & selectors(\"/tmp/dark.css\"))))
"' > /tmp/selectors.$$
version=$(head -1 /tmp/selectors.$$)
[ -n "$version" ] || { echo "no Shell version found in $image"; rm -f /tmp/selectors.$$; exit 1; }
tail -n +2 /tmp/selectors.$$ > "$out/shell-selectors-ubuntu-$version.txt"
rm -f /tmp/selectors.$$
echo "Ubuntu, GNOME Shell $version: $(wc -l < "$out/shell-selectors-ubuntu-$version.txt") selectors"
