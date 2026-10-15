#!/usr/bin/env python3
"""Recolored pictures and cursors must load in other programs' readers.

  scripts/pictures_accept.py icons <set folder> <accent #rrggbb>
  scripts/pictures_accept.py cursors <set folder> <dark #rrggbb> <light #rrggbb>

icons: GdkPixbuf, the reader GTK uses, must load every PNG in the set,
and the folder's body must be the accent. cursors: every file in the set
must be an Xcursor file whose pictures all lie inside it, read here
without Velatura's code, and the arrow's outline and body must be the
theme's light and dark colours.
"""
import os
import struct
import sys

problems = []
mode, folder = sys.argv[1:3]


def rgb(h):
    return tuple(int(h[i:i + 2], 16) for i in (1, 3, 5))


def near(a, b, by=3):
    return all(abs(x - y) <= by for x, y in zip(a, b))


if mode == "icons":
    import gi
    gi.require_version("GdkPixbuf", "2.0")
    from gi.repository import GdkPixbuf
    accent = rgb(sys.argv[3])
    count = 0
    for root, _, files in os.walk(folder):
        for f in files:
            if not f.endswith(".png"):
                continue
            count += 1
            try:
                GdkPixbuf.Pixbuf.new_from_file(os.path.join(root, f))
            except Exception as e:  # noqa: BLE001
                problems.append(f"{f} does not load: {e}")
    p = GdkPixbuf.Pixbuf.new_from_file(os.path.join(folder, "48/places/folder.png"))
    d, rs, n = p.get_pixels(), p.get_rowstride(), p.get_n_channels()
    body = tuple(d[12 * rs + 10 * n:12 * rs + 10 * n + 3])
    if not near(body, accent):
        problems.append(f"the folder's body is {body}, not the accent {accent}")
    what = f"{count} pictures load in GdkPixbuf, the folder in the accent"
else:
    dark, light = rgb(sys.argv[3]), rgb(sys.argv[4])
    names = sorted(os.listdir(os.path.join(folder, "cursors")))
    for name in names:
        data = open(os.path.join(folder, "cursors", name), "rb").read()
        if data[:4] != b"Xcur":
            problems.append(f"{name} is not an Xcursor file")
            continue
        header, _, count = struct.unpack_from("<III", data, 4)
        for i in range(count):
            kind, _, at = struct.unpack_from("<III", data, header + 12 * i)
            if kind == 0xFFFD0002:
                w, h = struct.unpack_from("<II", data, at + 16)
                if at + 36 + 4 * w * h > len(data):
                    problems.append(f"{name}: a picture runs past the end")
    data = open(os.path.join(folder, "cursors", "left_ptr"), "rb").read()
    at = 16 + 12 * 2 + 36

    def pixel(i):
        b, g, r, _ = data[at + 4 * i:at + 4 * i + 4]
        return (r, g, b)
    if not near(pixel(0), light) or not near(pixel(2 * 24 + 1), dark):
        problems.append(f"the arrow is {pixel(0)} on {pixel(2 * 24 + 1)}, not {light} on {dark}")
    what = f"{len(names)} cursors read, the arrow in the theme's colours"
if problems:
    print("\n".join(problems))
    sys.exit(1)
print(what)
