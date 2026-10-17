// Loaded only in the throw-away headless Shell started by shell-accept.sh.
// Reports what the panel and a checked quick settings toggle actually
// compute, so the script can tell whether the generated theme was loaded
// and applied, and whether the toggle has the theme's accent itself.
// For the top bar's style, also the panel's background with its alpha,
// and the fill of the quick settings button and of the clock's label;
// and how the UPS indicator stands.
import GLib from 'gi://GLib';
import St from 'gi://St';

import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';

const hex = c => '#' + [c.red, c.green, c.blue].map(v => v.toString(16).padStart(2, '0')).join('');
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
                const clock = Main.panel.statusArea.dateMenu?._clockDisplay;
                GLib.file_set_contents(`${out}/top-bar.txt`,
                    `${hexa(node.get_background_color())} ` +
                    `${fill(Main.panel.statusArea.quickSettings)} ${fill(clock)}\n`);
            } catch (e) {
                GLib.file_set_contents(`${out}/error.txt`, `${e}\n`);
            }
            return GLib.SOURCE_CONTINUE;
        });
    }

    disable() {
        GLib.source_remove(this._timer);
        this._toggle.destroy();
    }
}
