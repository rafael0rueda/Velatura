#!/usr/bin/env python3
"""GTK must take icons from a recolored set, and the rest from the theme
it was made from.

  scripts/icons_accept.py <icons folder> <set name> <base name> <accent #rrggbb>

Looks icons up the way an app does, with the set as the icon theme: the
folder icon must come from the set, and an icon the set does not hold from
elsewhere. Every icon in the set must load, and its folder must show the
accent.
"""
import colorsys
import os
import sys

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Gdk", "4.0")
from gi.repository import Gdk, Gtk  # noqa: E402

root, name, base, accent = sys.argv[1:5]
system = [d + "/icons" for d in os.environ.get("XDG_DATA_DIRS", "/usr/local/share:/usr/share").split(":") if d]
theme = Gtk.IconTheme.new()
theme.set_search_path([root, *system])
theme.set_theme_name(name)
problems = []


def where(icon):
    paintable = theme.lookup_icon(icon, None, 64, 1, Gtk.TextDirection.LTR, 0)
    file = paintable.get_file()
    return file.get_path() if file else ""


folder = where("folder")
if not folder.startswith(os.path.join(root, name) + "/"):
    problems.append(f"the folder icon comes from {folder}, not from the set")
other = where("text-x-generic")
if other.startswith(os.path.join(root, name) + "/") or not other:
    problems.append(f"an icon the set does not hold comes from {other or 'nowhere'}")

count = 0
for dirpath, _, files in os.walk(os.path.join(root, name)):
    for f in files:
        if f.endswith(".svg"):
            try:
                Gdk.Texture.new_from_filename(os.path.join(dirpath, f))
                count += 1
            except Exception as e:  # noqa: BLE001
                problems.append(f"{f} does not load: {e}")

# The colour that covers most of the folder, as drawn, has the accent's hue.
# (Not its exact value: a folder is several shades of one colour.)
gi.require_version("GdkPixbuf", "2.0")
from gi.repository import GdkPixbuf  # noqa: E402

pixbuf = GdkPixbuf.Pixbuf.new_from_file_at_size(folder, 64, 64)
px, stride, n = pixbuf.get_pixels(), pixbuf.get_rowstride(), pixbuf.get_n_channels()
counts = {}
for y in range(pixbuf.get_height()):
    for x in range(pixbuf.get_width()):
        i = y * stride + x * n
        if n == 4 and px[i + 3] < 250:
            continue
        rgb = tuple(px[i:i + 3])
        counts[rgb] = counts.get(rgb, 0) + 1
drawn = max(counts, key=counts.get)
want = tuple(int(accent[i:i + 2], 16) / 255 for i in (1, 3, 5))
h1, _, s1 = colorsys.rgb_to_hsv(*[c / 255 for c in drawn])
h2, _, _ = colorsys.rgb_to_hsv(*want)
gap = min(abs(h1 - h2), 1 - abs(h1 - h2)) * 360
if s1 < 0.15 or gap > 30:
    problems.append(f"the folder is drawn mostly in #{bytes(drawn).hex()}, not in the hue of the accent {accent}")

if problems:
    print(f"GTK does not take the set {name} as it should:")
    for p in problems:
        print(f"  {p}")
    sys.exit(1)
print(f"GTK takes {count} recolored icons from {name}, and the rest from {base} and those it inherits")
