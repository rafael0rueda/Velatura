#!/usr/bin/env python3
"""GtkSourceView must accept the style schemes the renderer produces.

  scripts/sourceview_accept.py <scheme.xml>...

Each file is put, as velatura.xml, in a folder that is the only place a
fresh scheme manager looks. The scheme must be found under its id, say
whether it is light or dark, and give every style the file sets with the
colours the file gives it.
"""
import re
import shutil
import sys
import tempfile

import gi

gi.require_version("GtkSource", "5")
from gi.repository import GLib, GtkSource  # noqa: E402

failed = False
# A warning from the parser is a failure: it would mean a style was dropped.
warnings = []
GLib.log_set_writer_func(
    lambda level, fields, _n=None, _d=None: (
        warnings.append(level) if level & (GLib.LogLevelFlags.LEVEL_WARNING | GLib.LogLevelFlags.LEVEL_CRITICAL) else None,
        GLib.LogWriterOutput.HANDLED,
    )[1]
)

for path in sys.argv[1:]:
    text = open(path, encoding="utf-8").read()
    with tempfile.TemporaryDirectory() as folder:
        shutil.copy(path, f"{folder}/velatura.xml")
        manager = GtkSource.StyleSchemeManager.new()
        manager.set_search_path([folder])
        scheme = manager.get_scheme("velatura")
        problems = []
        if scheme is None:
            problems.append("not found under the id velatura")
        else:
            variant = re.search(r'<property name="variant">(\w+)</property>', text).group(1)
            if scheme.get_metadata("variant") != variant:
                problems.append(f"variant is {scheme.get_metadata('variant')}, not {variant}")
            styles = re.findall(r'<style name="([^"]+)"([^>]*)/>', text)
            for name, attributes in styles:
                style = scheme.get_style(name)
                if style is None:
                    problems.append(f"style {name} is missing")
                    continue
                for key, value in re.findall(r'(foreground|background)="([^"]+)"', attributes):
                    got = style.get_property(key)
                    if (got or "").lower() != value:
                        problems.append(f"{name} {key} is {got}, not {value}")
            if not styles:
                problems.append("no styles in the file")
        if warnings:
            problems.append(f"{len(warnings)} warning(s) while reading it")
            warnings.clear()
    if problems:
        failed = True
        print(f"GtkSourceView rejects {path}:")
        for p in problems[:10]:
            print(f"  {p}")
    else:
        print(f"GtkSourceView accepts {path}: {len(styles)} styles")

sys.exit(1 if failed else 0)
