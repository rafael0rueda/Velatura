#!/usr/bin/env bash
# Build everything a release holds, into dist/ (or the folder given):
#
#   velatura-<version>-x86_64-linux.tar.gz   the command and the app, built
#       here, with install.sh to put them in ~/.local without root
#   velatura-<version>-source.tar.gz         the source with every crate it
#       needs, so it builds with no network (cargo build --offline)
#   velatura@rafael0rueda.github.io.shell-extension.zip   the Shell extension
#   SHA256SUMS                               a checksum for each
#
#   scripts/release.sh [output folder]
#
# The release workflow runs this on the oldest GNOME Velatura supports, so
# the programs also run on every newer one. It publishes nothing itself.
set -euo pipefail
cd "$(dirname "$0")/.."
out=$(realpath -m "${1:-dist}")
version=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1)
[ -n "$version" ] || { echo "no version in Cargo.toml"; exit 1; }
id=io.github.rafael0rueda.Velatura
rm -rf "$out"
mkdir -p "$out"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# The programs.
cargo build --release --locked -p velatura-cli -p velatura-gui
bin=velatura-$version-x86_64-linux
stage=$work/$bin
install -D -m 755 target/release/velatura "$stage/bin/velatura"
install -D -m 755 target/release/velatura-app "$stage/bin/velatura-app"
install -D -m 644 "data/$id.desktop" "$stage/share/applications/$id.desktop"
install -D -m 644 "data/icons/hicolor/scalable/apps/$id.svg" \
    "$stage/share/icons/hicolor/scalable/apps/$id.svg"
install -D -m 644 "data/icons/hicolor/symbolic/apps/$id-symbolic.svg" \
    "$stage/share/icons/hicolor/symbolic/apps/$id-symbolic.svg"
install -m 755 scripts/release-install.sh "$stage/install.sh"
install -m 644 LICENSE README.md "$stage/"
# The same bytes for the same sources: fixed times, owners and order.
when=$(git log -1 --format=%ct)
tar --sort=name --mtime="@$when" --owner=0 --group=0 --numeric-owner \
    -C "$work" -czf "$out/$bin.tar.gz" "$bin"

# The source, with every crate it needs.
src=velatura-$version-source
git archive --format=tar --prefix="$src/" HEAD | tar -x -C "$work"
(cd "$work/$src" && mkdir -p .cargo && cargo vendor --locked --versioned-dirs vendor > .cargo/config.toml)
tar --sort=name --mtime="@$when" --owner=0 --group=0 --numeric-owner \
    -C "$work" -czf "$out/$src.tar.gz" "$src"

scripts/pack-extension.sh "$out" >/dev/null
(cd "$out" && sha256sum -- *.tar.gz *.zip > SHA256SUMS)
echo "Velatura $version in $out:"
(cd "$out" && ls -l --block-size=K -- * | awk '{print "  " $5 "  " $9}')
