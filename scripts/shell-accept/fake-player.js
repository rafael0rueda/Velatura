// A stand-in for a media player, for shell-accept.sh only: one track by
// MPRIS on the session bus, as org.mpris.MediaPlayer2.velatura-test, playing
// until PlayPause is asked. It has no app, so the pill shows the generic
// audio icon.
//
//     gjs -m fake-player.js
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';

const xml = `<node>
 <interface name="org.mpris.MediaPlayer2">
  <method name="Raise"/>
  <property name="CanRaise" type="b" access="read"/>
  <property name="Identity" type="s" access="read"/>
  <property name="DesktopEntry" type="s" access="read"/>
 </interface>
 <interface name="org.mpris.MediaPlayer2.Player">
  <method name="PlayPause"/><method name="Next"/><method name="Previous"/>
  <property name="PlaybackStatus" type="s" access="read"/>
  <property name="Metadata" type="a{sv}" access="read"/>
  <property name="CanGoNext" type="b" access="read"/>
  <property name="CanGoPrevious" type="b" access="read"/>
  <property name="CanPlay" type="b" access="read"/>
 </interface>
</node>`;

let status = 'Playing';
const root = {CanRaise: false, Identity: 'Test Player', DesktopEntry: '', Raise() {}};
const player = {
    get PlaybackStatus() {
        return status;
    },
    get Metadata() {
        return {
            'mpris:trackid': new GLib.Variant('o', '/velatura/test/1'),
            'xesam:title': new GLib.Variant('s', 'A Test Track'),
            'xesam:artist': new GLib.Variant('as', ['The Testers']),
        };
    },
    CanGoNext: true,
    CanGoPrevious: false,
    CanPlay: true,
    PlayPause() {
        status = status === 'Playing' ? 'Paused' : 'Playing';
        playerObject.emit_property_changed('PlaybackStatus', new GLib.Variant('s', status));
    },
    Next() {},
    Previous() {},
};
const info = Gio.DBusNodeInfo.new_for_xml(xml);
const rootObject = Gio.DBusExportedObject.wrapJSObject(info.interfaces[0], root);
const playerObject = Gio.DBusExportedObject.wrapJSObject(info.interfaces[1], player);
rootObject.export(Gio.DBus.session, '/org/mpris/MediaPlayer2');
playerObject.export(Gio.DBus.session, '/org/mpris/MediaPlayer2');
Gio.bus_own_name(Gio.BusType.SESSION, 'org.mpris.MediaPlayer2.velatura-test',
    Gio.BusNameOwnerFlags.NONE, null, null, null);
new GLib.MainLoop(null, false).run();
