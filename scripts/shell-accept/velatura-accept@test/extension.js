// Loaded only in the throw-away headless Shell started by shell-accept.sh.
// Reports what the panel and a checked quick settings toggle actually
// compute, so the script can tell whether the generated theme was loaded
// and applied, and whether the toggle has the theme's accent itself.
import GLib from 'gi://GLib';
import St from 'gi://St';

import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';

const hex = c => '#' + [c.red, c.green, c.blue].map(v => v.toString(16).padStart(2, '0')).join('');

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
