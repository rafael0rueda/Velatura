#!/usr/bin/env bash
# Build the velatura command and the app from this checkout and install them
# for this user only: the two programs in ~/.local/bin, the app's launcher
# and icons under ~/.local/share. No root needed.
# Run again after every merged phase to keep the copy current.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --release --locked -p velatura-cli -p velatura-gui
install -D -m 755 target/release/velatura "$HOME/.local/bin/velatura"
install -D -m 755 target/release/velatura-app "$HOME/.local/bin/velatura-app"
id=io.github.rafael0rueda.Velatura
share=${XDG_DATA_HOME:-$HOME/.local/share}
install -D -m 644 "data/$id.desktop" "$share/applications/$id.desktop"
install -D -m 644 "data/icons/hicolor/scalable/apps/$id.svg" \
    "$share/icons/hicolor/scalable/apps/$id.svg"
install -D -m 644 "data/icons/hicolor/symbolic/apps/$id-symbolic.svg" \
    "$share/icons/hicolor/symbolic/apps/$id-symbolic.svg"
echo "Installed: $("$HOME/.local/bin/velatura" --version) from $(git describe --tags --always --dirty)"
case ":$PATH:" in *":$HOME/.local/bin:"*) ;; *) echo "Note: ~/.local/bin is not on PATH";; esac
