//! Ferramari window-state helper for GNOME Shell.
//!
//! GNOME does not expose native Wayland toplevels through EWMH, and
//! `org.gnome.Shell.Introspect.GetWindows` is allowlist-gated. This tiny
//! extension runs inside the compositor and answers one question over the
//! session bus: for each monitor, is it fully covered or showing a fullscreen
//! window? Ferramari polls it and feeds the result into the same Eco policy
//! the X11 EWMH watcher uses.
//!
//! The payload deliberately contains no window titles, PIDs or app ids.

import Gio from 'gi://Gio';
import Meta from 'gi://Meta';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import { Extension } from 'resource:///org/gnome/shell/extensions/extension.js';

const SERVICE = 'io.github.ferramari.Ferramari.WindowState';
const PATH = '/io/github/ferramari/Ferramari/WindowState';
const IFACE = `<node>
  <interface name="io.github.ferramari.Ferramari.WindowState">
    <method name="GetState">
      <arg type="s" direction="out" name="json"/>
    </method>
  </interface>
</node>`;

// Meta.Window has changed accessor names across releases. Read-style helpers
// keep the extension working on GNOME 45–50 instead of one method call taking
// the whole state down.
function isFullscreen(window) {
    if (typeof window.is_fullscreen === 'function')
        return window.is_fullscreen();
    return Boolean(window.fullscreen);
}

function isMaximized(window) {
    if (typeof window.get_maximized === 'function')
        return window.get_maximized() !== 0;
    return Boolean(window.maximized_horizontally || window.maximized_vertically);
}

function skipsTaskbar(window) {
    if (typeof window.is_skip_taskbar === 'function')
        return window.is_skip_taskbar();
    return Boolean(window.skip_taskbar);
}

function isOnAllWorkspaces(window) {
    if (typeof window.is_on_all_workspaces === 'function')
        return window.is_on_all_workspaces();
    return Boolean(window.on_all_workspaces);
}

export default class FerramariWindowStateExtension extends Extension {
    enable() {
        this._exported = Gio.DBusExportedObject.wrapJSObject(IFACE, this);
        this._exported.export(Gio.DBus.session, PATH);
        this._nameId = Gio.bus_own_name_on_connection(
            Gio.DBus.session,
            SERVICE,
            Gio.BusNameOwnerFlags.NONE,
            null,
            null);
    }

    disable() {
        if (this._nameId) {
            Gio.bus_unown_name(this._nameId);
            this._nameId = 0;
        }
        if (this._exported) {
            this._exported.unexport();
            this._exported = null;
        }
    }

    GetState() {
        return JSON.stringify(this._state());
    }

    _state() {
        try {
            const activeWorkspace = global.workspace_manager.get_active_workspace();
            const windows = global.get_window_actors()
                .map(actor => actor.meta_window)
                .filter(window => {
                    if (!window || window.minimized)
                        return false;
                    // Never count the desktop itself: Ferramari's own X11
                    // surface is a DESKTOP window that covers a whole monitor,
                    // and counting it would pause the video it is showing.
                    const type = typeof window.get_window_type === 'function'
                        ? window.get_window_type()
                        : null;
                    if (type === Meta.WindowType.DESKTOP ||
                        type === Meta.WindowType.DOCK ||
                        type === Meta.WindowType.PANEL)
                        return false;
                    // Panels and docks skip the taskbar; a fullscreen game may
                    // set the same hint, so only skip non-fullscreen windows.
                    if (skipsTaskbar(window) && !isFullscreen(window))
                        return false;
                    if (isOnAllWorkspaces(window))
                        return true;
                    const workspace = typeof window.get_workspace === 'function'
                        ? window.get_workspace()
                        : null;
                    return workspace === activeWorkspace;
                });

            const monitors = Main.layoutManager.monitors.map((monitor, fallbackIndex) => {
                const geometry = {
                    x: monitor.x,
                    y: monitor.y,
                    width: monitor.width,
                    height: monitor.height,
                };
                let covered = false;
                let fullscreen = false;
                for (const window of windows) {
                    let rect;
                    try {
                        rect = window.get_frame_rect();
                    } catch {
                        continue;
                    }
                    const centerX = rect.x + Math.floor(rect.width / 2);
                    const centerY = rect.y + Math.floor(rect.height / 2);
                    const centerInside = centerX >= geometry.x &&
                        centerX < geometry.x + geometry.width &&
                        centerY >= geometry.y &&
                        centerY < geometry.y + geometry.height;
                    const covers = rect.x <= geometry.x &&
                        rect.y <= geometry.y &&
                        rect.x + rect.width >= geometry.x + geometry.width &&
                        rect.y + rect.height >= geometry.y + geometry.height;
                    if (covers || (isMaximized(window) && centerInside))
                        covered = true;
                    if (isFullscreen(window) && (covers || centerInside)) {
                        covered = true;
                        fullscreen = true;
                    }
                }
                return {
                    index: monitor.index ?? fallbackIndex,
                    connector: monitor.connector ?? null,
                    ...geometry,
                    covered,
                    fullscreen,
                };
            });

            return { monitors };
        } catch (error) {
            // A compositor API change must not take down the shell; an empty
            // list simply lets Ferramari keep its X11 fallback/current state.
            console.error(`[ferramari-window-state] ${error}`);
            return { monitors: [] };
        }
    }
}
