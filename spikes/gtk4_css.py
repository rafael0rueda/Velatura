#!/usr/bin/env python3
"""Phase 0 spike: how GTK4/libadwaita treat a user gtk.css.

Each scenario runs in its own process with a throw-away XDG_CONFIG_HOME, so
the real ~/.config is never read or written. No window is shown.

    gtk4_css.py            run every scenario, print one line each
    gtk4_css.py <name>     run one scenario (used internally)
"""
import os
import subprocess
import sys
import tempfile
from pathlib import Path

THEME = "#aa0000"   # what Velatura wants
USER = "#111111"    # what the user's own gtk.css says
STOCK = "#fafafb"   # what the app forces for itself

SCENARIOS = {
    # name: (description, files relative to the config dir, expected colour or None)
    "import_top_user_after": (
        "@import at top, user's own definition below it",
        {"gtk-4.0/gtk.css": f'@import url("velatura.css");\n@define-color window_bg_color {USER};\n',
         "gtk-4.0/velatura.css": f"@define-color window_bg_color {THEME};\n"}),
    "import_bottom": (
        "user's definition first, @import as the last line",
        {"gtk-4.0/gtk.css": f'@define-color window_bg_color {USER};\n@import url("velatura.css");\n',
         "gtk-4.0/velatura.css": f"@define-color window_bg_color {THEME};\n"}),
    "import_after_rule": (
        "a style rule, then @import",
        {"gtk-4.0/gtk.css": f'window {{ margin: 0; }}\n@define-color window_bg_color {USER};\n@import url("velatura.css");\n',
         "gtk-4.0/velatura.css": f"@define-color window_bg_color {THEME};\n"}),
    "inline_block_bottom": (
        "user's definition first, Velatura's definitions inline at the end",
        {"gtk-4.0/gtk.css": f"@define-color window_bg_color {USER};\n/* velatura:begin */\n@define-color window_bg_color {THEME};\n/* velatura:end */\n"}),
    "import_only": (
        "gtk.css contains only the @import",
        {"gtk-4.0/gtk.css": '@import url("velatura.css");\n',
         "gtk-4.0/velatura.css": f"@define-color window_bg_color {THEME};\n"}),
    "symlinked_gtk_css": (
        "gtk.css is a symlink into a dotfiles dir; velatura.css sits beside the link",
        {"dotfiles/gtk.css": '@import url("velatura.css");\n',
         "gtk-4.0/gtk.css": "->../dotfiles/gtk.css",
         "gtk-4.0/velatura.css": f"@define-color window_bg_color {THEME};\n"}),
    "media_contrast": (
        "definition wrapped in @media (prefers-contrast: no-preference)",
        {"gtk-4.0/gtk.css": f"@media (prefers-contrast: no-preference) {{\n@define-color window_bg_color {THEME};\n}}\n"}),
    "media_unknown": (
        "an unknown @media feature before a plain definition",
        {"gtk-4.0/gtk.css": f"@media (velatura-nonsense: 1) {{ window {{ margin: 0; }} }}\n@define-color window_bg_color {THEME};\n"}),
    "root_variable": (
        ":root variable only, no named colour",
        {"gtk-4.0/gtk.css": f":root {{ --window-bg-color: {THEME}; }}\n"}),
    "app_immune": (
        "user gtk.css themes the app; the app adds its own provider above user priority",
        {"gtk-4.0/gtk.css": f"@define-color window_bg_color {THEME};\n:root {{ --window-bg-color: {THEME}; }}\n"}),
    "scoped_preview": (
        "a subtree with its own colours for the preview",
        {}),
}


def hexof(rgba):
    return "#%02x%02x%02x" % tuple(round(c * 255) for c in (rgba.red, rgba.green, rgba.blue))


def run_one(name):
    import gi
    gi.require_version("Gtk", "4.0")
    gi.require_version("Adw", "1")
    from gi.repository import Adw, Gdk, Gtk

    Adw.init()
    display = Gdk.Display.get_default()
    win = Gtk.Window()
    label = Gtk.Label(label="x")
    box = Gtk.Box()
    box.append(label)
    win.set_child(box)

    if name == "app_immune":
        p = Gtk.CssProvider()
        p.load_from_string(
            f"@define-color window_bg_color {STOCK};\n:root {{ --window-bg-color: {STOCK}; }}\n")
        Gtk.StyleContext.add_provider_for_display(
            display, p, Gtk.STYLE_PROVIDER_PRIORITY_USER + 1)
    if name == "scoped_preview":
        p = Gtk.CssProvider()
        p.load_from_string(
            f".vel-preview {{ --window-fg-color: {THEME}; }}\n"
            ".vel-preview label { color: var(--window-fg-color); }\n")
        Gtk.StyleContext.add_provider_for_display(
            display, p, Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION)
        box.add_css_class("vel-preview")
        print("label=" + hexof(label.get_color()))
        return

    found, rgba = win.get_style_context().lookup_color("window_bg_color")
    out = "named=" + (hexof(rgba) if found else "unset")
    # the CSS variable, as a widget actually computes it
    p = Gtk.CssProvider()
    p.load_from_string("label.vel-probe { color: var(--window-bg-color); }")
    Gtk.StyleContext.add_provider_for_display(display, p, Gtk.STYLE_PROVIDER_PRIORITY_USER + 50)
    label.add_css_class("vel-probe")
    print(out + " variable=" + hexof(label.get_color()))


def main():
    if len(sys.argv) > 1:
        return run_one(sys.argv[1])
    import gi
    gi.require_version("Gtk", "4.0")
    gi.require_version("Adw", "1")
    from gi.repository import Adw, Gtk
    print(f"GTK {Gtk.MAJOR_VERSION}.{Gtk.MINOR_VERSION}.{Gtk.MICRO_VERSION}, "
          f"libadwaita {Adw.MAJOR_VERSION}.{Adw.MINOR_VERSION}.{Adw.MICRO_VERSION}")
    for name, (desc, files) in SCENARIOS.items():
        with tempfile.TemporaryDirectory(prefix="velatura-spike-") as tmp:
            cfg = Path(tmp) / "config"
            for rel, content in files.items():
                path = cfg / rel
                path.parent.mkdir(parents=True, exist_ok=True)
                if content.startswith("->"):
                    path.symlink_to(content[2:])
                else:
                    path.write_text(content)
            cfg.mkdir(parents=True, exist_ok=True)
            env = dict(os.environ, XDG_CONFIG_HOME=str(cfg), GTK_THEME="", GTK_DEBUG="")
            env.pop("GTK_THEME")
            r = subprocess.run([sys.executable, __file__, name], env=env,
                               capture_output=True, text=True, timeout=60)
            warn = [line for line in r.stderr.splitlines() if "gtk.css" in line or "velatura.css" in line]
            print(f"{name:24} {r.stdout.strip() or 'NO OUTPUT'}"
                  + (f"   [warnings: {len(warn)}: {warn[0][-110:]}]" if warn else "")
                  + f"\n{'':24} ({desc})")


if __name__ == "__main__":
    main()
