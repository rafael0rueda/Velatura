#!/usr/bin/env bash
# Pack Velatura's Shell extension the way extensions.gnome.org takes it: a
# zip with metadata.json and extension.js at its top level.
#
#   scripts/pack-extension.sh [output folder]
#
# Uploading it is the owner's to do, signed in at extensions.gnome.org.
set -euo pipefail
cd "$(dirname "$0")/.."
uuid=velatura@rafael0rueda.github.io
out=$(realpath -m "${1:-target}")
mkdir -p "$out"
python3 - "$uuid" "$out" <<'PY'
import json, sys, zipfile
uuid, out = sys.argv[1], sys.argv[2]
src = f"extension/{uuid}"
meta = json.load(open(f"{src}/metadata.json"))
assert meta["uuid"] == uuid, "the folder and the uuid must agree"
path = f"{out}/{uuid}.shell-extension.zip"
with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
    for name in ("metadata.json", "extension.js"):
        # A fixed date, so the same source always gives the same file.
        info = zipfile.ZipInfo(name, date_time=(2026, 1, 1, 0, 0, 0))
        info.external_attr = 0o644 << 16
        z.writestr(info, open(f"{src}/{name}", "rb").read(), zipfile.ZIP_DEFLATED)
print(path)
PY
