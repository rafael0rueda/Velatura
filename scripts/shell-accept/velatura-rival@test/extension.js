// Loaded only in the throw-away headless Shell started by shell-accept.sh.
// Does nothing itself: its stylesheet.css is what the test is about.
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

export default class Rival extends Extension {
    enable() {}

    disable() {}
}
