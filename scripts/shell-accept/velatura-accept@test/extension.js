// Loaded only in the throw-away headless Shell started by shell-accept.sh.
// Reports what the panel actually computes, so the script can tell whether
// the generated theme was loaded and applied.
import GLib from 'gi://GLib';

import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';

const hex = c => '#' + [c.red, c.green, c.blue].map(v => v.toString(16).padStart(2, '0')).join('');

export default class Probe extends Extension {
    enable() {
        const out = GLib.getenv('VEL_OUT');
        this._timer = GLib.timeout_add(GLib.PRIORITY_DEFAULT, 500, () => {
            try {
                const node = Main.panel.get_theme_node();
                GLib.file_set_contents(`${out}/panel.txt`,
                    `${hex(node.get_background_color())} ${hex(node.get_foreground_color())}\n`);
            } catch (e) {
                GLib.file_set_contents(`${out}/error.txt`, `${e}\n`);
            }
            return GLib.SOURCE_CONTINUE;
        });
    }

    disable() {
        GLib.source_remove(this._timer);
    }
}
