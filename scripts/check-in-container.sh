#!/usr/bin/env bash
# Run the gate in the same Fedora container GitHub Actions uses. Needs podman,
# no root. Use this when a tool the gate needs is not installed on the host.
#
#   scripts/check-in-container.sh          run the gate
#   scripts/check-in-container.sh --fmt    reformat the sources first
set -euo pipefail
cd "$(dirname "$0")/.."
fmt=""
[ "${1:-}" = "--fmt" ] && fmt="cargo fmt --all &&"
exec podman run --rm --security-opt label=disable -v "$PWD":/src -w /src \
    -e CARGO_TARGET_DIR=/tmp/target -e VELATURA_CI=1 fedora:44 bash -ec "
    dnf install -y -q --setopt=install_weak_deps=False git cargo rustfmt clippy gcc python3 pkgconf-pkg-config glib2-devel dbus-daemon dconf gsettings-desktop-schemas gtk3 gtk4 python3-gobject gobject-introspection gtk4-devel libadwaita-devel gettext desktop-file-utils >/dev/null
    $fmt scripts/check.sh"
