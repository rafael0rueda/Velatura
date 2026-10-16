// Velatura's companion extension.
//
// It loads the stylesheets the Velatura app writes, and loads them again
// when those files change. All colours are in the files; nothing here
// changes with a theme, so the extension never needs updating for one.
//
// It also keeps an outline beside the focused window, which shows only
// once a stylesheet gives it a border: the app writes that one while its
// Window Outline part is on. Maximized and full-screen windows get none.
//
// It starts no programs, holds no settings and patches nothing in the
// Shell.
//
// SPDX-License-Identifier: GPL-3.0-or-later

import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import Meta from 'gi://Meta';
import St from 'gi://St';

import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';

// The contract with the app: two files, and the outline's style class.
const STYLESHEET = ['velatura', 'shell', 'gnome-shell.css'];
const OUTLINE_SHEET = ['velatura', 'shell', 'window-outline.css'];
const OUTLINE_CLASS = 'velatura-window-outline';

const OUTLINED = [
    Meta.WindowType.NORMAL,
    Meta.WindowType.DIALOG,
    Meta.WindowType.MODAL_DIALOG,
];

// One stylesheet the app writes, loaded while it is there.
class Sheet {
    constructor(parts) {
        this._file = Gio.File.new_for_path(
            GLib.build_filenamev([GLib.get_user_data_dir(), ...parts]));
        this._loaded = false;
        this._pending = 0;
        this._monitor = this._file.monitor_file(Gio.FileMonitorFlags.NONE, null);
        this._changedId = this._monitor.connect('changed', () => this._soon());
        this._reload();
    }

    // Replacing a file is several events; act once when they settle.
    _soon() {
        if (this._pending)
            return;
        this._pending = GLib.timeout_add(GLib.PRIORITY_DEFAULT, 150, () => {
            this._pending = 0;
            this._reload();
            return GLib.SOURCE_REMOVE;
        });
    }

    destroy() {
        if (this._pending) {
            GLib.source_remove(this._pending);
            this._pending = 0;
        }
        this._monitor.disconnect(this._changedId);
        this._monitor.cancel();
        this._monitor = null;
        this._unload();
        this._file = null;
    }

    _theme() {
        return St.ThemeContext.get_for_stage(global.stage).get_theme();
    }

    _unload() {
        if (this._loaded) {
            this._theme().unload_stylesheet(this._file);
            this._loaded = false;
        }
    }

    _reload() {
        this._unload();
        if (!this._file.query_exists(null))
            return;
        try {
            this._theme().load_stylesheet(this._file);
            this._loaded = true;
        } catch (e) {
            // One syntax error rejects the whole file; the Shell keeps its
            // own look and carries on.
            console.error(`Velatura: ${this._file.get_path()} was not loaded: ${e.message}`);
        }
    }
}

// The outline: a frame with no colour of its own, kept just outside the
// focused window and stacked right above it.
class Outline {
    constructor() {
        this._widget = new St.Widget({style_class: OUTLINE_CLASS, reactive: false, visible: false});
        global.window_group.add_child(this._widget);
        this._window = null;
        this._windowIds = [];
        this._later = 0;
        this._ids = [
            [this._widget, this._widget.connect('style-changed', () => this._queue())],
            [global.display, global.display.connect('notify::focus-window', () => this._follow())],
            [global.display, global.display.connect('restacked', () => this._queue())],
            [Main.overview, Main.overview.connect('showing', () => this._queue())],
            [Main.overview, Main.overview.connect('hidden', () => this._queue())],
        ];
        this._follow();
    }

    destroy() {
        if (this._later) {
            global.compositor.get_laters().remove(this._later);
            this._later = 0;
        }
        this._leave();
        for (const [object, id] of this._ids)
            object.disconnect(id);
        this._ids = [];
        this._widget.destroy();
        this._widget = null;
    }

    _leave() {
        for (const id of this._windowIds)
            this._window.disconnect(id);
        this._windowIds = [];
        this._window = null;
    }

    _follow() {
        if (this._window)
            this._leave();
        this._window = global.display.focus_window;
        if (this._window) {
            this._windowIds = [
                'position-changed', 'size-changed', 'notify::fullscreen',
                'notify::minimized', 'workspace-changed',
            ].map(signal => this._window.connect(signal, () => this._queue()));
            this._windowIds.push(this._window.connect('unmanaged', () => {
                this._leave();
                this._queue();
            }));
        }
        this._queue();
    }

    // Placed just before the next frame, once the Shell has stacked the
    // windows: placed at once, a window raised in the same change (as when
    // leaving a full-screen one) could end up above the outline.
    _queue() {
        if (this._later)
            return;
        this._later = global.compositor.get_laters().add(Meta.LaterType.BEFORE_REDRAW, () => {
            this._later = 0;
            this._place();
            return GLib.SOURCE_REMOVE;
        });
    }

    _maximized(window) {
        // GNOME 49 has is_maximized(); earlier ones have get_maximized().
        if (window.is_maximized)
            return window.is_maximized();
        return window.get_maximized() === Meta.MaximizeFlags.BOTH;
    }

    _actor() {
        const window = this._window;
        if (!window || Main.overview.visible || window.minimized)
            return null;
        if (!OUTLINED.includes(window.get_window_type()))
            return null;
        if (window.is_fullscreen() || this._maximized(window))
            return null;
        const actor = window.get_compositor_private();
        if (!actor || !actor.visible || actor.get_parent() !== global.window_group)
            return null;
        return actor;
    }

    _place() {
        const widget = this._widget;
        const actor = this._actor();
        // The border's width is the stylesheet's; none means no outline.
        const width = actor ? widget.get_theme_node().get_border_width(St.Side.TOP) : 0;
        if (!actor || width <= 0) {
            widget.hide();
            return;
        }
        const rect = this._window.get_frame_rect();
        widget.set_position(rect.x - width, rect.y - width);
        widget.set_size(rect.width + 2 * width, rect.height + 2 * width);
        global.window_group.set_child_above_sibling(widget, actor);
        widget.show();
    }
}

export default class Velatura extends Extension {
    enable() {
        this._sheets = [new Sheet(STYLESHEET), new Sheet(OUTLINE_SHEET)];
        this._outline = new Outline();
    }

    disable() {
        this._outline.destroy();
        this._outline = null;
        for (const sheet of this._sheets)
            sheet.destroy();
        this._sheets = null;
    }
}
