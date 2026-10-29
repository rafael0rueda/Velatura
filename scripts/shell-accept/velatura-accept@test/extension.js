// Loaded only in the throw-away headless Shell started by shell-accept.sh.
// Reports what the panel and a checked quick settings toggle actually
// compute, so the script can tell whether the generated theme was loaded
// and applied, and whether the toggle has the theme's accent itself.
// For the top bar's style, also the panel's background with its alpha,
// and the fill of the quick settings button and of the clock's label;
// and how the UPS indicator, the Tasks checklist and the media player
// stand, adding a task through the checklist's entry, and clicking the
// media player with a pointer of its own, when asked to. And two things the Shell's own
// stylesheet hides behind `!important`: the background of a menu's
// separator, an insensitive item, and the fill of the clock's button
// itself on hover.
import Clutter from 'gi://Clutter';
import GLib from 'gi://GLib';
import St from 'gi://St';

import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';

const hex = c => '#' + [c.red, c.green, c.blue].map(v => v.toString(16).padStart(2, '0')).join('');
// The first actor under `actor` that `test` holds for, or null.
const find = (actor, test) => {
    if (test(actor))
        return actor;
    for (const child of actor.get_children()) {
        const found = find(child, test);
        if (found)
            return found;
    }
    return null;
};
const hexa = c => hex(c) + c.alpha.toString(16).padStart(2, '0');
// The inset fill a panel button is drawn with, or "none".
const fill = actor => {
    const shadow = actor?.get_theme_node().get_box_shadow();
    return shadow ? hexa(shadow.color) : 'none';
};

export default class Probe extends Extension {
    enable() {
        const out = GLib.getenv('VEL_OUT');
        // A toggle of the kind quick settings shows, switched on, out of
        // sight but on the stage, where its style is worked out.
        this._toggle = new St.Button({style_class: 'quick-toggle', opacity: 0});
        this._toggle.add_style_pseudo_class('checked');
        Main.uiGroup.add_child(this._toggle);
        this._separator = new St.BoxLayout({style_class: 'popup-menu-item', opacity: 0});
        this._separator.add_style_pseudo_class('insensitive');
        Main.uiGroup.add_child(this._separator);
        this._clockButton = new St.Bin({style_class: 'panel-button clock-display', opacity: 0});
        this._clockButton.add_style_pseudo_class('hover');
        Main.panel._rightBox.add_child(this._clockButton);
        this._timer = GLib.timeout_add(GLib.PRIORITY_DEFAULT, 500, () => {
            try {
                const node = Main.panel.get_theme_node();
                const toggle = this._toggle.get_theme_node();
                GLib.file_set_contents(`${out}/panel.txt`,
                    `${hex(node.get_background_color())} ${hex(node.get_foreground_color())} ` +
                    `${hex(toggle.get_background_color())}\n`);
                // The Shell starts in the overview, where the bar is
                // transparent; asked to, the probe leaves it.
                if (GLib.getenv('VEL_NO_OVERVIEW') && Main.overview.visible)
                    Main.overview.hide();
                // The UPS indicator: shown or not, its watts, its state,
                // and the colour of its watts.
                const ups = Main.panel.statusArea['velatura-ups'];
                const watts = ups?.first_child?.get_children().find(a => a instanceof St.Label);
                const state = ['on-battery', 'low-battery'].find(c => ups?.has_style_class_name(c)) ?? 'normal';
                GLib.file_set_contents(`${out}/ups.txt`, ups
                    ? `${ups.container.visible ? 'shown' : 'hidden'} ${watts?.text.replace(' ', '') || '-'} ${state} ` +
                      `${watts ? hex(watts.get_theme_node().get_foreground_color()) : '-'}\n`
                    : 'none\n');
                // The checklist: shown or not, its count, which side of the
                // clock it is on, and the colour of a done task. A task
                // written to add-task is added through its entry.
                const tasks = Main.panel.statusArea['velatura-tasks'];
                if (tasks) {
                    const center = Main.panel._centerBox.get_children();
                    const side = center.indexOf(tasks.container) <
                        center.indexOf(Main.panel.statusArea.dateMenu.container) ? 'left' : 'right';
                    const count = tasks.first_child.get_children().find(a => a instanceof St.Label);
                    const done = find(tasks.menu.box, a => a.has_style_class_name?.('done'));
                    GLib.file_set_contents(`${out}/tasks.txt`,
                        `${tasks.container.visible ? 'shown' : 'hidden'} ${count.text || '0'} ` +
                        `${side}-of-clock ${done ? hex(done.get_theme_node().get_foreground_color()) : '-'}\n`);
                    const add = `${out}/add-task`;
                    if (GLib.file_test(add, GLib.FileTest.EXISTS)) {
                        const [, text] = GLib.file_get_contents(add);
                        GLib.unlink(add);
                        const entry = find(tasks.menu.box, a => a instanceof St.Entry);
                        entry.set_text(new TextDecoder().decode(text).trim());
                        entry.clutter_text.emit('activate');
                    }
                } else {
                    GLib.file_set_contents(`${out}/tasks.txt`, 'none\n');
                }
                // The media player: shown or not, whether it plays, which
                // side of the clock it is on, whether its menu is open, and
                // the colour of its play state. A button written to
                // click-media clicks the middle of it.
                const media = Main.panel.statusArea['velatura-media'];
                if (media) {
                    const center = Main.panel._centerBox.get_children();
                    const side = center.indexOf(media.container) <
                        center.indexOf(Main.panel.statusArea.dateMenu.container) ? 'left' : 'right';
                    const playing = media._active ? (media._active.playing ? 'playing' : 'paused') : '-';
                    const state = find(media, a => a.has_style_class_name?.('velatura-media-state'));
                    GLib.file_set_contents(`${out}/media.txt`,
                        `${media.container.visible ? 'shown' : 'hidden'} ${playing} ${side}-of-clock ` +
                        `${media.menu.isOpen ? 'open' : 'closed'} ` +
                        `${hex(state.get_theme_node().get_foreground_color())}\n`);
                    const click = `${out}/click-media`;
                    if (GLib.file_test(click, GLib.FileTest.EXISTS)) {
                        const [, button] = GLib.file_get_contents(click);
                        GLib.unlink(click);
                        this._click(media, Number(new TextDecoder().decode(button)));
                    }
                } else {
                    GLib.file_set_contents(`${out}/media.txt`, 'none\n');
                }
                const clock = Main.panel.statusArea.dateMenu?._clockDisplay;
                GLib.file_set_contents(`${out}/top-bar.txt`,
                    `${hexa(node.get_background_color())} ` +
                    `${fill(Main.panel.statusArea.quickSettings)} ${fill(clock)}\n`);
                GLib.file_set_contents(`${out}/hidden.txt`,
                    `${hexa(this._separator.get_theme_node().get_background_color())} ` +
                    `${fill(this._clockButton)}\n`);
            } catch (e) {
                GLib.file_set_contents(`${out}/error.txt`, `${e}\n`);
            }
            return GLib.SOURCE_CONTINUE;
        });
    }

    // A press and a release of `button` in the middle of `actor`, a moment
    // apart, through a pointer of its own, so gestures see a real click.
    // The pointer is made at the first click and not before: added while a
    // menu is open, an input device closes it, and resting at the corner it
    // would hover the toggle measured above. A new one drops what it is
    // sent at once, so its first click waits a little.
    _click(actor, button) {
        let wait = 0;
        if (!this._pointer) {
            const seat = global.stage.context.get_backend().get_default_seat();
            this._pointer = seat.create_virtual_device(Clutter.InputDeviceType.POINTER_DEVICE);
            wait = 500;
        }
        const [x, y] = actor.get_transformed_position();
        const now = () => GLib.get_monotonic_time();
        const later = (ms, step) => GLib.timeout_add(GLib.PRIORITY_DEFAULT, wait + ms, () => {
            step();
            return GLib.SOURCE_REMOVE;
        });
        later(0, () => this._pointer.notify_absolute_motion(now(), x + actor.width / 2, y + actor.height / 2));
        later(100, () => this._pointer.notify_button(now(), button, Clutter.ButtonState.PRESSED));
        later(180, () => this._pointer.notify_button(now(), button, Clutter.ButtonState.RELEASED));
    }

    disable() {
        GLib.source_remove(this._timer);
        this._toggle.destroy();
        this._separator.destroy();
        this._clockButton.destroy();
    }
}
