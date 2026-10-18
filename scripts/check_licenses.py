#!/usr/bin/env python3
"""Every third-party crate must carry a licence compatible with GPL-3.0.

Reads `cargo metadata`; no network access beyond what the build needs.
"""
import json
import re
import subprocess
import sys

ALLOWED = {
    "MIT", "Apache-2.0", "Apache-2.0 WITH LLVM-exception", "BSD-2-Clause",
    "BSD-3-Clause", "ISC", "Zlib", "Unicode-3.0", "Unicode-DFS-2016",
    "Unlicense", "0BSD", "CC0-1.0", "LGPL-2.1-or-later", "LGPL-3.0-or-later",
    "GPL-3.0-or-later", "MPL-2.0", "BSL-1.0",
}


def acceptable(expr: str) -> bool:
    """An SPDX expression as SPDX reads it: AND binds tighter than OR,
    parentheses group, "A WITH B" is one licence, and the old "A/B" is OR.
    "A OR B" needs one acceptable side; "A AND B" needs both."""
    tokens = re.findall(r"\(|\)|[^\s()]+", expr.replace("/", " OR "))
    at = 0

    def peek():
        return tokens[at] if at < len(tokens) else None

    def take():
        nonlocal at
        at += 1
        return tokens[at - 1]

    def either():
        ok = both()
        while peek() == "OR":
            take()
            ok = both() or ok
        return ok

    def both():
        ok = one()
        while peek() == "AND":
            take()
            ok = one() and ok
        return ok

    def one():
        token = take() if peek() is not None else None
        if token == "(":
            ok = either()
            if take() != ")":
                raise ValueError(expr)
            return ok
        if token in (None, ")", "AND", "OR", "WITH"):
            raise ValueError(expr)
        if peek() == "WITH":
            take()
            token = f"{token} WITH {take()}"
        return token in ALLOWED

    try:
        ok = either()
        return ok and at == len(tokens)
    except (ValueError, IndexError):
        return False


# The rules above, held to: a stricter licence ANDed onto a choice is not
# lifted by the choice.
for expr, want in [
    ("MIT OR Apache-2.0", True),
    ("MIT/Apache-2.0", True),
    ("(MIT OR Apache-2.0) AND Unicode-3.0", True),
    ("(MIT OR Apache-2.0) AND SSPL-1.0", False),
    ("MIT AND SSPL-1.0 OR Apache-2.0", True),
    ("Apache-2.0 WITH LLVM-exception OR MIT", True),
    ("GPL-2.0-only", False),
    ("", False),
    ("(MIT", False),
]:
    assert acceptable(expr) == want, expr


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
