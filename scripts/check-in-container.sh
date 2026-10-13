#!/usr/bin/env bash
# Run the gate in a Fedora container, as GitHub Actions does. Needs podman,
# no root. Use this when a tool the gate needs is not installed on the host,
# or to check against another GNOME version than the host's.
#
#   scripts/check-in-container.sh              the gate, as CI runs it
#   scripts/check-in-container.sh --fmt        reformat the sources first
#   scripts/check-in-container.sh --desktop    also install GNOME Shell, so
#                                              the app, colour and Shell
#                                              checks run headless
#   VELATURA_IMAGE=fedora:45 scripts/check-in-container.sh --desktop
#                                              the same on GNOME 51
#   VELATURA_RUSTUP=1 VELATURA_IMAGE=fedora:41 scripts/check-in-container.sh --desktop
#                                              on a release whose own Rust is
#                                              too old to build with: take the
#                                              current toolchain from rustup
set -euo pipefail
cd "$(dirname "$0")/.."
fmt=""; desktop=""
for a in "$@"; do
    case "$a" in --fmt) fmt="cargo fmt --all &&" ;; --desktop) desktop=1 ;; esac
done
image=${VELATURA_IMAGE:-fedora:44}
packages=$(tr '\n' ' ' < scripts/ci-packages.txt)
[ -n "$desktop" ] && packages+=" $(tr '\n' ' ' < scripts/ci-packages-desktop.txt)"
exec podman run --rm --security-opt label=disable -v "$PWD":/src -w /src \
    -e CARGO_TARGET_DIR=/tmp/target -e VELATURA_CI=1 ${desktop:+-e VELATURA_DESKTOP=1} "$image" bash -ec "
    dnf install -y -q --setopt=install_weak_deps=False $packages >/dev/null
    if [ -n \"${VELATURA_RUSTUP:-}\" ]; then
        dnf install -y -q --setopt=install_weak_deps=False rustup >/dev/null
        rustup-init -y -q --profile minimal --component clippy,rustfmt >/dev/null 2>&1
        . \"\$HOME/.cargo/env\"
    fi
    $fmt scripts/check.sh"
