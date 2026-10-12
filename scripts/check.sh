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

step "docs"
python3 scripts/check_docs.py

if [ ${#skipped[@]} -gt 0 ]; then
    printf '\nPASSED, with skipped steps: %s\n' "${skipped[*]}"
else
    printf '\nPASSED\n'
fi
