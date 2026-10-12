// Phase 0 spike driver: loaded only in the throw-away headless Shell started
// by run.sh. Walks through User Themes scenarios and records what the panel
// actually computes.
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';

import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';

const sleep = ms => new Promise(r => GLib.timeout_add(GLib.PRIORITY_DEFAULT, ms, () => {
    r();
    return GLib.SOURCE_REMOVE;
}));

const hex = c => '#' + [c.red, c.green, c.blue].map(v => v.toString(16).padStart(2, '0')).join('') +
    (c.alpha === 255 ? '' : `/${c.alpha}`);

export default class Probe extends Extension {
    enable() {
        const out = GLib.getenv('VEL_OUT');
        this._run(out).catch(e => GLib.file_set_contents(`${out}/error.txt`, `${e}\n${e.stack}`));
    }

    disable() {}

    async _run(out) {
        const themes = GLib.getenv('VEL_THEMES');
        const src = Gio.SettingsSchemaSource.new_from_directory(
            GLib.getenv('VEL_USER_THEME_SCHEMAS'), Gio.SettingsSchemaSource.get_default(), false);
        const ut = new Gio.Settings({
            settings_schema: src.lookup('org.gnome.shell.extensions.user-theme', false)});
        const iface = new Gio.Settings({schema_id: 'org.gnome.desktop.interface'});
        const a11y = new Gio.Settings({schema_id: 'org.gnome.desktop.a11y.interface'});
        const css = (name, text) => GLib.file_set_contents(
            `${themes}/${name}/gnome-shell/gnome-shell.css`, text);
        const results = [];
        const record = async label => {
            await sleep(1500);
            const node = Main.panel.get_theme_node();
            results.push({label, panel_bg: hex(node.get_background_color()),
                panel_height: Main.panel.height});
        };

        await sleep(4000);
        Main.overview.hide();
        await record('stock, dark');
        ut.set_string('name', 'Velatura-a');
        await record('theme a (#aa0000), overrides only');
        css('Velatura-a', '#panel { background-color: #00aa00; }\n');
        await record('theme a rewritten to #00aa00, same name');
        ut.set_string('name', 'Velatura-a');
        await record('same name set again');
        ut.set_string('name', 'Velatura-b');
        await record('theme b (#0000aa), new name');
        a11y.set_boolean('high-contrast', true);
        await record('theme b + high contrast on');
        a11y.set_boolean('high-contrast', false);
        iface.set_string('color-scheme', 'default');
        await record('theme b + light style');
        iface.set_string('color-scheme', 'prefer-dark');
        ut.set_string('name', 'Velatura-bad');
        await record('malformed stylesheet, then a valid rule (#123456)');
        ut.set_string('name', '');
        await record('theme name cleared');

        GLib.file_set_contents(`${out}/results.json`, JSON.stringify(results, null, 1));
        GLib.file_set_contents(`${out}/done`, '');
    }
}
