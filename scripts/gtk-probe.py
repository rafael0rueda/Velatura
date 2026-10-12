#!/usr/bin/env python3
"""GTK probe: a libadwaita app must compute the colours of a generated
stylesheet, and they must win over definitions earlier in the user's file.

    scripts/gtk-probe.py <generated velatura.css>

Works in a throw-away config folder; opens no window. Needs a display.
"""
import os
import re
import sys
import tempfile

generated = open(sys.argv[1]).read()
expected = dict(re.findall(r"@define-color (\w+) (#[0-9a-f]{6});", generated))
tmp = tempfile.mkdtemp()
os.makedirs(f"{tmp}/gtk-4.0")
open(f"{tmp}/gtk-4.0/velatura.css", "w").write(generated)
open(f"{tmp}/gtk-4.0/gtk.css", "w").write(
    "@define-color window_bg_color #ff00ff;\n@define-color accent_bg_color #ff00ff;\n"
    '/* >>> velatura gtk4 1 >>> */\n@import url("velatura.css");\n/* <<< velatura gtk4 1 <<< */\n'
)
os.environ["XDG_CONFIG_HOME"] = tmp
os.environ.setdefault("GTK_A11Y", "none")

import warnings  # noqa: E402

import gi  # noqa: E402

gi.require_version("Gtk", "4.0")
gi.require_version("Adw", "1")
from gi.repository import Adw, Gtk  # noqa: E402

warnings.simplefilter("ignore")
Adw.init()
context = Gtk.Window().get_style_context()
wrong = []
for name, want in sorted(expected.items()):
    ok, c = context.lookup_color(name)
    got = "#%02x%02x%02x" % tuple(round(v * 255) for v in (c.red, c.green, c.blue)) if ok else None
    if got != want:
        wrong.append(f"  {name}: expected {want}, computed {got}")
if wrong:
    print(f"libadwaita computes other colours than {sys.argv[1]} defines:")
    print("\n".join(wrong))
    sys.exit(1)
print(f"libadwaita computes all {len(expected)} opaque colours of {sys.argv[1]}")
