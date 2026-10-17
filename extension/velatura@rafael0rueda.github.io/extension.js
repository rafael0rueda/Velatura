// Velatura's companion extension.
//
// It loads the stylesheets the Velatura app writes, and loads them again
// when those files change: the Shell's colours, the top bar's style and
// the window outline's colour. All colours are in the files; nothing here
// changes with a theme, so the extension never needs updating for one.
//
// It also keeps an outline beside the focused window, which shows only
// once a stylesheet gives it a border: the app writes that one while its
// Window Outline part is on. Maximized and full-screen windows get none.
//
// While the app's UPS part is on, it shows the watts a UPS draws in the
// top bar, with a menu of its state. It reads them every few seconds from
// Network UPS Tools on this computer, speaking NUT's own text protocol,
// and shows nothing while no UPS answers. The look is in the app's file.
//
// It starts no programs, reaches nothing beyond this computer, holds no
// settings and patches nothing in the Shell.
//
// SPDX-License-Identifier: GPL-3.0-or-later

import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import Clutter from 'gi://Clutter';
import GObject from 'gi://GObject';
import Meta from 'gi://Meta';
import St from 'gi://St';

import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import * as PanelMenu from 'resource:///org/gnome/shell/ui/panelMenu.js';
import * as PopupMenu from 'resource:///org/gnome/shell/ui/popupMenu.js';

// The contract with the app: four files, and two style classes.
const STYLESHEET = ['velatura', 'shell', 'gnome-shell.css'];
const TOP_BAR_SHEET = ['velatura', 'shell', 'top-bar.css'];
const UPS_SHEET = ['velatura', 'shell', 'ups.css'];
const OUTLINE_SHEET = ['velatura', 'shell', 'window-outline.css'];
const OUTLINE_CLASS = 'velatura-window-outline';
const UPS_CLASS = 'velatura-ups';

// NUT's server on this computer, and how often it is asked.
const NUT_HOST = 'localhost';
const NUT_PORT = 3493;
const POLL_SECONDS = 5;
// A reading may take this long and send this much, many times what a UPS
// with every variable NUT knows needs, and is given up past either. Each
// read of the socket has its own limit too, but one byte at a time would
// keep that from ever running out.
const NUT_DEADLINE_MS = 3000;
const NUT_MAX_BYTES = 64 * 1024;

// GNOME 48 gave St.BoxLayout an orientation in place of `vertical`.
const VERTICAL = GObject.Object.find_property.call(St.BoxLayout, 'orientation')
    ? {orientation: Clutter.Orientation.VERTICAL} : {vertical: true};

Gio._promisify(Gio.SocketClient.prototype, 'connect_to_host_async');
Gio._promisify(Gio.OutputStream.prototype, 'write_bytes_async');
Gio._promisify(Gio.InputStream.prototype, 'read_bytes_async');

const OUTLINED = [
    Meta.WindowType.NORMAL,
    Meta.WindowType.DIALOG,
    Meta.WindowType.MODAL_DIALOG,
];

// One stylesheet the app writes, loaded while it is there. `changed` is
// told after each load whether the file is loaded.
class Sheet {
    constructor(parts, changed = null) {
        this._file = Gio.File.new_for_path(
            GLib.build_filenamev([GLib.get_user_data_dir(), ...parts]));
        this._changed = changed;
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
        this._changed = null;
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
        if (this._file.query_exists(null)) {
            try {
                this._theme().load_stylesheet(this._file);
                this._loaded = true;
            } catch (e) {
                // One syntax error rejects the whole file; the Shell keeps
                // its own look and carries on.
                console.error(`Velatura: ${this._file.get_path()} was not loaded: ${e.message}`);
            }
        }
        this._changed?.(this._loaded);
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

// A line of NUT's protocol as its words: spaces part them, double quotes
// hold spaces, and a backslash keeps the character after it.
function words(line) {
    const out = [];
    let word = null;
    let quoted = false;
    for (let i = 0; i < line.length; i++) {
        const ch = line[i];
        if (ch === '\\' && i + 1 < line.length) {
            word = (word ?? '') + line[++i];
        } else if (ch === '"') {
            quoted = !quoted;
            word ??= '';
        } else if (ch === ' ' && !quoted) {
            if (word !== null)
                out.push(word);
            word = null;
        } else {
            word = (word ?? '') + ch;
        }
    }
    if (word !== null)
        out.push(word);
    return out;
}

// The lines NUT sends, read in pieces so that nothing it sends, one
// endless line included, is held past NUT_MAX_BYTES.
class Lines {
    constructor(stream, cancellable) {
        this._stream = stream;
        this._cancellable = cancellable;
        this._buffer = new Uint8Array(0);
        this._total = 0;
    }

    // The next line without its line ending, or null at the end.
    async next() {
        for (;;) {
            const end = this._buffer.indexOf(10);
            if (end >= 0) {
                const line = this._buffer.subarray(0, end);
                this._buffer = this._buffer.slice(end + 1);
                return new TextDecoder().decode(line).replace(/\r$/, '');
            }
            if (this._total > NUT_MAX_BYTES)
                throw new Error('NUT answered too much');
            const bytes = await this._stream.read_bytes_async(4096, GLib.PRIORITY_DEFAULT,
                this._cancellable);
            const piece = bytes.toArray();
            if (piece.length === 0)
                return null;
            this._total += piece.length;
            const joined = new Uint8Array(this._buffer.length + piece.length);
            joined.set(this._buffer);
            joined.set(piece, this._buffer.length);
            this._buffer = joined;
        }
    }
}

// What NUT says of the first UPS it lists, as name to value, or null if
// there is none. One connection per reading, closed at the end; the
// reading is cancelled with `cancellable`, or past NUT_DEADLINE_MS.
async function readUps(cancellable) {
    const reading = new Gio.Cancellable();
    // g_cancellable_connect(): called at once if already cancelled.
    const linked = cancellable.connect(() => reading.cancel());
    let deadline = GLib.timeout_add(GLib.PRIORITY_DEFAULT, NUT_DEADLINE_MS, () => {
        deadline = 0;
        reading.cancel();
        return GLib.SOURCE_REMOVE;
    });
    try {
        return await askUps(reading);
    } finally {
        if (deadline)
            GLib.source_remove(deadline);
        cancellable.disconnect(linked);
    }
}

async function askUps(cancellable) {
    const client = new Gio.SocketClient({timeout: 3});
    const connection = await client.connect_to_host_async(NUT_HOST, NUT_PORT, cancellable);
    try {
        const input = new Lines(connection.get_input_stream(), cancellable);
        const output = connection.get_output_stream();
        // One question, and the lines of its answer up to its END line.
        const ask = async question => {
            await output.write_bytes_async(new GLib.Bytes(new TextEncoder().encode(`${question}\n`)),
                GLib.PRIORITY_DEFAULT, cancellable);
            const lines = [];
            for (;;) {
                const line = await input.next();
                if (line === null)
                    throw new Error('NUT closed the connection');
                if (line.startsWith('ERR '))
                    throw new Error(`NUT answered ${line}`);
                if (line.startsWith('END '))
                    return lines.map(words);
                if (lines.length > 1000)
                    throw new Error('NUT answered too much');
                lines.push(line);
            }
        };
        // A UPS name is letters, figures and a few marks; anything else
        // is not asked about.
        const name = (await ask('LIST UPS')).find(w => w[0] === 'UPS')?.[1];
        if (!name || !/^[A-Za-z0-9_.-]+$/.test(name))
            return null;
        const vars = {};
        for (const w of await ask(`LIST VAR ${name}`)) {
            if (w[0] === 'VAR' && w.length >= 4)
                vars[w[2]] = w[3];
        }
        return vars;
    } finally {
        connection.close(null);
    }
}

function formatRuntime(seconds) {
    const minutes = Math.round(seconds / 60);
    return minutes < 60 ? `${minutes} min`
        : `${Math.floor(minutes / 60)} h ${minutes % 60} min`;
}

// The UPS in the top bar: its icon and the watts it draws, and a menu
// with its model, state, load, battery, runtime and input. Hidden until
// NUT answers with a reading of power.
const UpsIndicator = GObject.registerClass(
class UpsIndicator extends PanelMenu.Button {
    _init() {
        super._init(0.5, 'UPS');
        this.add_style_class_name(UPS_CLASS);
        const box = new St.BoxLayout({style_class: 'panel-status-menu-box'});
        box.add_child(new St.Icon({
            icon_name: 'uninterruptible-power-supply-symbolic',
            style_class: 'system-status-icon',
        }));
        this._watts = new St.Label({
            style_class: `${UPS_CLASS}-watts`,
            y_align: Clutter.ActorAlign.CENTER,
        });
        box.add_child(this._watts);
        this.add_child(box);

        this.menu.box.add_style_class_name(`${UPS_CLASS}-menu`);
        const item = new PopupMenu.PopupBaseMenuItem({reactive: false, can_focus: false});
        const card = new St.BoxLayout({
            style_class: `${UPS_CLASS}-card`,
            ...VERTICAL,
            x_expand: true,
        });
        item.add_child(card);
        this.menu.addMenuItem(item);
        this._model = new St.Label({style_class: `${UPS_CLASS}-title`});
        card.add_child(this._model);
        const rows = new St.BoxLayout({style_class: `${UPS_CLASS}-rows`, ...VERTICAL});
        card.add_child(rows);
        const row = key => {
            const line = new St.BoxLayout({style_class: `${UPS_CLASS}-row`});
            line.add_child(new St.Label({text: key, style_class: `${UPS_CLASS}-key`, x_expand: true}));
            const value = new St.Label({style_class: `${UPS_CLASS}-value`});
            line.add_child(value);
            rows.add_child(line);
            return value;
        };
        this._status = row('Status');
        this._load = row('Load');
        this._battery = row('Battery');
        this._runtime = row('Runtime');
        this._input = row('Input');

        this._cancel = new Gio.Cancellable();
        this._busy = false;
        this._poll();
        this._timer = GLib.timeout_add_seconds(GLib.PRIORITY_DEFAULT, POLL_SECONDS, () => {
            this._poll();
            return GLib.SOURCE_CONTINUE;
        });
        // However it is destroyed, by the extension or along with the panel.
        this.connect('destroy', () => {
            GLib.source_remove(this._timer);
            this._timer = 0;
            this._cancel.cancel();
        });
    }

    async _poll() {
        if (this._busy)
            return;
        this._busy = true;
        let vars = null;
        try {
            vars = await readUps(this._cancel);
        } catch {
            // Not there, not answering in time, or not making sense: hidden.
        } finally {
            this._busy = false;
        }
        // Taken away while NUT was being read.
        if (!this._cancel.is_cancelled())
            this._show(vars);
    }

    _show(v) {
        const watts = Number(v?.['ups.realpower'] ?? v?.['ups.power']);
        this.container.visible = Number.isFinite(watts);
        if (!this.container.visible) {
            this.menu.close();
            return;
        }
        const flags = (v['ups.status'] ?? '').split(' ');
        const low = flags.includes('LB');
        const battery = flags.includes('OB');
        this.remove_style_class_name('on-battery');
        this.remove_style_class_name('low-battery');
        if (low)
            this.add_style_class_name('low-battery');
        else if (battery)
            this.add_style_class_name('on-battery');
        const status = low ? 'Battery low' : battery ? 'On battery' : 'On AC power';
        const shown = `${Math.round(watts)} W`;
        this._watts.text = shown;
        this.accessible_name = `UPS: ${status}, ${shown}`;

        this._model.text = v['device.model'] ?? v['ups.model'] ?? 'UPS';
        this._status.text = status;
        const max = v['ups.realpower.nominal'];
        this._load.text = shown + (max ? ` of ${max} W` : '') +
            (v['ups.load'] ? `  ·  ${v['ups.load']}%` : '');
        this._battery.text = v['battery.charge'] ? `${v['battery.charge']}%` : '—';
        this._runtime.text = v['battery.runtime'] ? formatRuntime(Number(v['battery.runtime'])) : '—';
        this._input.text = v['input.voltage'] ? `${v['input.voltage']} V` : '—';
    }
});

export default class Velatura extends Extension {
    enable() {
        this._ups = null;
        this._sheets = [
            new Sheet(STYLESHEET),
            new Sheet(TOP_BAR_SHEET),
            new Sheet(UPS_SHEET, loaded => this._showUps(loaded)),
            new Sheet(OUTLINE_SHEET),
        ];
        this._outline = new Outline();
    }

    // The UPS indicator lives while its stylesheet is there: in the left
    // of the bar, after the workspaces. Hidden until NUT answers.
    _showUps(loaded) {
        if (loaded && !this._ups) {
            this._ups = new UpsIndicator();
            Main.panel.addToStatusArea(UPS_CLASS, this._ups, 1, 'left');
            this._ups.container.hide();
        } else if (!loaded && this._ups) {
            this._ups.destroy();
            this._ups = null;
        }
    }

    disable() {
        this._ups?.destroy();
        this._ups = null;
        this._outline.destroy();
        this._outline = null;
        for (const sheet of this._sheets)
            sheet.destroy();
        this._sheets = null;
    }
}
