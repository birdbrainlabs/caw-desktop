// For Caw's automatic tests only (never shipped): the headless test session
// needs org.gnome.Shell.Eval and the screenshot interface, which only work in
// unsafe mode.
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

export default class CawTest extends Extension {
    enable() {
        global.context.unsafe_mode = true;
    }

    disable() {}
}
