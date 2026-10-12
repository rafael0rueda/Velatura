#!/usr/bin/env bash
# Collect every translatable string of the app into po/velatura.pot.
# Run after changing text in crates/velatura-gui. Needs gettext 0.24 or
# newer, the first to read Rust.
set -euo pipefail
cd "$(dirname "$0")/.."
xgettext --language=Rust --from-code=UTF-8 --keyword=tr --keyword=plural:1,2 \
    --add-comments=Translators --package-name=velatura \
    --msgid-bugs-address=https://github.com/rafael0rueda/Velatura/issues \
    --copyright-holder="Rafael Rueda" --no-location \
    -o po/velatura.pot crates/velatura-gui/src/*.rs
sed -i '/^"POT-Creation-Date:/d' po/velatura.pot
echo "po/velatura.pot: $(grep -c '^msgid ' po/velatura.pot) strings"
