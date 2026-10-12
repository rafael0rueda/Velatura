#!/usr/bin/env bash
# Build the velatura command from this checkout and copy it to ~/.local/bin,
# so it works from any folder and from a text console. No root needed.
# Run again after every merged phase to keep the copy current.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --release --locked -p velatura-cli
install -D -m 755 target/release/velatura "$HOME/.local/bin/velatura"
echo "Installed: $("$HOME/.local/bin/velatura" --version) from $(git describe --tags --always --dirty)"
case ":$PATH:" in *":$HOME/.local/bin:"*) ;; *) echo "Note: ~/.local/bin is not on PATH";; esac
