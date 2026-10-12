#!/usr/bin/env python3
"""Phase 0 spike: does a GTK3 theme honour named-colour overrides from the
user's gtk-3.0/gtk.css?

    gtk3_css.py <themes-dir>     themes-dir holds adw-gtk3-dark/ (may be absent)

Runs each case in its own process with throw-away XDG dirs. No window is shown.
"""
import os
import subprocess
import sys
import tempfile
from pathlib import Path

THEME = "#aa0000"
NAMES = ("window_bg_color", "theme_bg_color")


def hexof(rgba):
    return "#%02x%02x%02x" % tuple(round(c * 255) for c in (rgba.red, rgba.green, rgba.blue))


def run_one():
    import gi
    gi.require_version("Gtk", "3.0")
    from gi.repository import Gtk
    win = Gtk.Window()
    button = Gtk.Button(label="x")
    win.add(button)
    ctx = win.get_style_context()
    bg = ctx.get_property("background-color", Gtk.StateFlags.NORMAL)
    print("theme=%s window-background=%s" % (
        Gtk.Settings.get_default().props.gtk_theme_name, hexof(bg)))


def main():
    if len(sys.argv) > 1 and sys.argv[1] == "--one":
        return run_one()
    themes = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else None
    cases = [("Adwaita", "stock Adwaita (dark)", "Adwaita:dark")]
    if themes and (themes / "adw-gtk3-dark").is_dir():
        cases.append(("adw-gtk3-dark", "adw-gtk3-dark", "adw-gtk3-dark"))
    for theme, label, gtk_theme in cases:
        for override in (False, True):
            with tempfile.TemporaryDirectory(prefix="velatura-spike-") as tmp:
                cfg, data = Path(tmp) / "config", Path(tmp) / "data"
                (cfg / "gtk-3.0").mkdir(parents=True)
                data.mkdir()
                if themes:
                    (data / "themes").symlink_to(themes)
                if override:
                    (cfg / "gtk-3.0/gtk.css").write_text(
                        "".join(f"@define-color {n} {THEME};\n" for n in NAMES))
                env = dict(os.environ, XDG_CONFIG_HOME=str(cfg), XDG_DATA_HOME=str(data),
                           GTK_THEME=gtk_theme)
                r = subprocess.run([sys.executable, __file__, "--one"], env=env,
                                   capture_output=True, text=True, timeout=60)
                print(f"{label:22} override={'yes' if override else 'no ':3}  {r.stdout.strip() or r.stderr.strip()[-200:]}")


if __name__ == "__main__":
    main()
