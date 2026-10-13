// Velatura's companion extension.
//
// It does one thing: load the stylesheet the Velatura app writes, and load
// it again when that file changes. All colours are in the file; nothing
// here changes with a theme, so the extension never needs updating for one.
// It starts no programs, holds no settings and patches nothing in the Shell.
//
// SPDX-License-Identifier: GPL-3.0-or-later

import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import St from 'gi://St';

import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

// The one-file contract with the app.
const STYLESHEET = ['velatura', 'shell', 'gnome-shell.css'];

export default class Velatura extends Extension {
    enable() {
        this._file = Gio.File.new_for_path(
            GLib.build_filenamev([GLib.get_user_data_dir(), ...STYLESHEET]));
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

    disable() {
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
