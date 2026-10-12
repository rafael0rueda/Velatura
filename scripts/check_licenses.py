#!/usr/bin/env python3
"""Every third-party crate must carry a licence compatible with GPL-3.0.

Reads `cargo metadata`; no network access beyond what the build needs.
"""
import json
import subprocess
import sys

ALLOWED = {
    "MIT", "Apache-2.0", "Apache-2.0 WITH LLVM-exception", "BSD-2-Clause",
    "BSD-3-Clause", "ISC", "Zlib", "Unicode-3.0", "Unicode-DFS-2016",
    "Unlicense", "0BSD", "CC0-1.0", "LGPL-2.1-or-later", "LGPL-3.0-or-later",
    "GPL-3.0-or-later", "MPL-2.0", "BSL-1.0",
}


def acceptable(expr: str) -> bool:
    # "A OR B" needs one acceptable side; "A AND B" needs both.
    expr = expr.replace("/", " OR ").replace("(", " ").replace(")", " ")
    return any(
        all(part.strip() in ALLOWED for part in alt.split(" AND "))
        for alt in expr.split(" OR ")
    )


meta = json.loads(subprocess.check_output(
    ["cargo", "metadata", "--format-version", "1", "--locked"]))
bad = []
count = 0
for pkg in meta["packages"]:
    if pkg["source"] is None:
        continue
    count += 1
    lic = pkg.get("license") or ""
    if not acceptable(lic):
        bad.append(f'{pkg["name"]} {pkg["version"]}: {lic or "no licence stated"}')
if bad:
    print("licences not on the allowed list:")
    print("\n".join("  " + b for b in bad))
    sys.exit(1)
print(f"licences: {count} crates checked, ok")
