#!/usr/bin/env python3
"""GTK acceptance: GTK itself must load every generated stylesheet without
a parsing error or warning.

    scripts/gtk_accept.py <3|4> <file.css>...

Runs one GTK major version per process, because both cannot be loaded into
the same one. Parsing needs no display.
"""
import sys

import gi

major = sys.argv[1]
gi.require_version("Gtk", f"{major}.0")
from gi.repository import Gtk  # noqa: E402

failed = False
for path in sys.argv[2:]:
    problems = []
    provider = Gtk.CssProvider()
    provider.connect(
        "parsing-error",
        lambda _p, section, error: problems.append(
            f"line {section.get_start_location().lines + 1 if major == '4' else section.get_start_line() + 1}: {error.message}"
        ),
    )
    with open(path, "rb") as f:
        data = f.read()
    try:
        if major == "4":
            provider.load_from_string(data.decode())
        else:
            provider.load_from_data(data)
    except Exception as e:  # GTK3 raises on the first error
        problems.append(str(e))
    if problems:
        failed = True
        print(f"GTK {major} rejects {path}:")
        for p in problems:
            print(f"  {p}")
    else:
        print(f"GTK {major} accepts {path}")
sys.exit(1 if failed else 0)
