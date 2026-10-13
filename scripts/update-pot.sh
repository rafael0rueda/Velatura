#!/usr/bin/env bash
# Collect every translatable string of the app, and the sentences it shares
# with the command, into po/velatura.pot.
# Run after changing text in crates/velatura-gui. Needs gettext 0.24 or
# newer, the first to read Rust.
set -euo pipefail
cd "$(dirname "$0")/.."
xgettext --language=Rust --from-code=UTF-8 --keyword=tr --keyword=plural:1,2 --keyword=t --keyword=tf! \
    --add-comments=Translators --no-wrap --package-name=velatura \
    --msgid-bugs-address=https://github.com/rafael0rueda/Velatura/issues \
    --copyright-holder="Rafael Rueda" --no-location \
    -o po/velatura.pot crates/velatura-gui/src/*.rs crates/velatura-text/src/*.rs
sed -i '/^"POT-Creation-Date:/d' po/velatura.pot
echo "po/velatura.pot: $(grep -c '^msgid ' po/velatura.pot) strings"
# Bring each language's catalog in step: new sentences appear in it with no
# translation yet, and ones no longer used leave it. A test fails until
# every sentence has its translation.
for lang in $(cat po/LINGUAS); do
    msgmerge --quiet --update --backup=none --no-fuzzy-matching --no-location --no-wrap \
        "po/$lang.po" po/velatura.pot
    sed -i '/^#~/d' "po/$lang.po"
    msgfmt -c -o /dev/null "po/$lang.po"
done
