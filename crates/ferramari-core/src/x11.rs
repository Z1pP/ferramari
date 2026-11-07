//! X11 window-state watcher.
//!
//! The same information is available straight
//! from EWMH properties, so this module polls `_NET_CLIENT_LIST` and friends
//! from a background thread. Polling keeps it robust: no event races, no
//! per-window signal bookkeeping, and windows disappearing mid-query is
//! harmless.

use crate::bus::{Command, CommandSender};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ChangeWindowAttributesAux, ConnectionExt, EventMask};
use x11rb::protocol::Event;

/// One visible top-level window, in root coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WindowRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub maximized: bool,
    pub fullscreen: bool,
}

/// All visible windows that can occlude a wallpaper. The list is global; the
/// Eco Engine evaluates it separately for every monitor.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WindowState {
    pub windows: Vec<WindowRect>,
}

/// What a window list says about one monitor rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MonitorVisibility {
    pub covered: bool,
    pub fullscreen: bool,
}

impl WindowState {
    /// Whether a monitor is completely covered by a normal window, and
    /// whether a fullscreen window is present. A maximised window counts as
    /// covered even when decorations/panels leave a few pixels around its
    /// client area: the user cannot see the wallpaper there either.
    pub fn visibility_at(&self, x: i32, y: i32, width: i32, height: i32) -> MonitorVisibility {
        if width <= 0 || height <= 0 {
            return MonitorVisibility::default();
        }
        let mut visibility = MonitorVisibility::default();
        for window in &self.windows {
            let center_inside = center_inside(window, x, y, width, height);
            let covers = rect_covers(window, x, y, width, height);
            if covers || (window.maximized && center_inside) {
                visibility.covered = true;
            }
            if window.fullscreen && (covers || center_inside) {
                visibility.covered = true;
                visibility.fullscreen = true;
            }
        }
        visibility
    }
}

fn rect_covers(window: &WindowRect, x: i32, y: i32, width: i32, height: i32) -> bool {
    window.width > 0
        && window.height > 0
        && window.x <= x
        && window.y <= y
        && window.x.saturating_add(window.width) >= x.saturating_add(width)
        && window.y.saturating_add(window.height) >= y.saturating_add(height)
}

fn center_inside(window: &WindowRect, x: i32, y: i32, width: i32, height: i32) -> bool {
    if window.width <= 0 || window.height <= 0 {
        return false;
    }
    let center_x = window.x.saturating_add(window.width / 2);
    let center_y = window.y.saturating_add(window.height / 2);
    center_x >= x
        && center_x < x.saturating_add(width)
        && center_y >= y
        && center_y < y.saturating_add(height)
}

pub struct Watcher {
    stop: Arc<AtomicBool>,
}

impl Watcher {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Start the EWMH watcher. Returns `None` when X11 is not reachable.
///
/// On a Wayland session the app still runs as an XWayland client, and every
/// X11 window does appear in `_NET_CLIENT_LIST` (measured: a fullscreen mpv
/// window is listed with `_NET_WM_STATE_FULLSCREEN`). Native Wayland windows
/// are not exposed through EWMH at all, so a native fullscreen app cannot be
/// detected there; that is a partial result, not a reason to disable the
/// watcher, which is why the earlier Wayland gate was removed.
pub fn start(tx: CommandSender) -> Option<Watcher> {
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    std::thread::Builder::new()
        .name("ferramari-x11".into())
        .spawn(move || {
            if let Err(err) = run(tx, thread_stop) {
                log::warn!("[WindowHandler] Disabled: {err}");
            }
        })
        .ok()?;
    Some(Watcher { stop })
}

fn run(tx: CommandSender, stop: Arc<AtomicBool>) -> anyhow::Result<()> {
    let (connection, screen) = x11rb::connect(None)?;
    let root = connection.setup().roots[screen].root;
    let atoms = Atoms::new(&connection)?;

    // Root property events tell us when a window appears/disappears or the
    // workspace changes. Client property/structure events tell us when a
    // visible window is maximised, resized or destroyed. No polling timer is
    // involved; an idle desktop produces no wakeups.
    connection.change_window_attributes(
        root,
        &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE),
    )?;
    connection.flush()?;

    let mut previous = None;
    let mut known_clients = std::collections::HashSet::new();
    refresh(
        &connection,
        root,
        &atoms,
        &tx,
        &mut previous,
        &mut known_clients,
    );

    while !stop.load(Ordering::Relaxed) {
        let event = connection.wait_for_event()?;
        let mut dirty = event_is_relevant(&event, &atoms);
        while let Some(event) = connection.poll_for_event()? {
            dirty |= event_is_relevant(&event, &atoms);
        }
        if dirty {
            refresh(
                &connection,
                root,
                &atoms,
                &tx,
                &mut previous,
                &mut known_clients,
            );
        }
    }
    Ok(())
}

fn event_is_relevant(event: &Event, atoms: &Atoms) -> bool {
    match event {
        Event::PropertyNotify(event) => {
            event.atom == atoms.wm_state
                || event.atom == atoms.client_list
                || event.atom == atoms.current_desktop
        }
        // StructureNotify on each client: mapping, resizing and destruction
        // can all change whether the wallpaper is covered.
        Event::ConfigureNotify(_)
        | Event::MapNotify(_)
        | Event::UnmapNotify(_)
        | Event::DestroyNotify(_) => true,
        _ => false,
    }
}

fn refresh(
    connection: &impl Connection,
    root: u32,
    atoms: &Atoms,
    tx: &CommandSender,
    previous: &mut Option<WindowState>,
    known_clients: &mut std::collections::HashSet<u32>,
) {
    // Subscribe before querying so a state change that lands between the
    // query and the next wait is not lost.
    if let Ok(clients) = property32(connection, root, atoms.client_list, AtomEnum::WINDOW) {
        known_clients.retain(|window| clients.contains(window));
        for window in clients.into_iter().filter(|window| *window != 0) {
            if known_clients.insert(window) {
                let _ = connection.change_window_attributes(
                    window,
                    &ChangeWindowAttributesAux::new()
                        .event_mask(EventMask::PROPERTY_CHANGE | EventMask::STRUCTURE_NOTIFY),
                );
            }
        }
    }
    match query(connection, root, atoms) {
        Ok(state) => {
            if previous.as_ref() != Some(&state) {
                log::debug!("[WindowHandler] {state:?}");
                *previous = Some(state.clone());
                let _ = tx.send(Command::WindowState(state));
            }
        }
        // A transient X error (a client vanishing mid-query, a slow
        // compositor) must not be mistaken for "nothing is maximised":
        // that would resume the wallpaper behind a fullscreen game.
        Err(err) => log::debug!("[WindowHandler] query failed ({err}); keeping {previous:?}"),
    }
}

struct Atoms {
    client_list: u32,
    current_desktop: u32,
    wm_state: u32,
    wm_desktop: u32,
    wm_window_type: u32,
    hidden: u32,
    maximized_vert: u32,
    maximized_horz: u32,
    fullscreen: u32,
    type_desktop: u32,
    type_dock: u32,
}

impl Atoms {
    fn new(connection: &impl Connection) -> anyhow::Result<Self> {
        let atom = |name: &str| -> anyhow::Result<u32> {
            Ok(connection
                .intern_atom(false, name.as_bytes())?
                .reply()?
                .atom)
        };
        Ok(Self {
            client_list: atom("_NET_CLIENT_LIST")?,
            current_desktop: atom("_NET_CURRENT_DESKTOP")?,
            wm_state: atom("_NET_WM_STATE")?,
            wm_desktop: atom("_NET_WM_DESKTOP")?,
            wm_window_type: atom("_NET_WM_WINDOW_TYPE")?,
            hidden: atom("_NET_WM_STATE_HIDDEN")?,
            maximized_vert: atom("_NET_WM_STATE_MAXIMIZED_VERT")?,
            maximized_horz: atom("_NET_WM_STATE_MAXIMIZED_HORZ")?,
            fullscreen: atom("_NET_WM_STATE_FULLSCREEN")?,
            type_desktop: atom("_NET_WM_WINDOW_TYPE_DESKTOP")?,
            type_dock: atom("_NET_WM_WINDOW_TYPE_DOCK")?,
        })
    }
}

fn query(connection: &impl Connection, root: u32, atoms: &Atoms) -> anyhow::Result<WindowState> {
    let clients = property32(connection, root, atoms.client_list, AtomEnum::WINDOW)?;
    let current_desktop = property32(connection, root, atoms.current_desktop, AtomEnum::CARDINAL)?
        .first()
        .copied();

    let mut windows = Vec::new();
    for window in clients.into_iter().filter(|window| *window != 0) {
        // A client can disappear between the list and this query. Skipping
        // one bad window keeps the rest of the state usable instead of
        // failing the whole sample.
        let Ok(types) = property32(connection, window, atoms.wm_window_type, AtomEnum::ATOM) else {
            continue;
        };
        if types.contains(&atoms.type_desktop) || types.contains(&atoms.type_dock) {
            continue;
        }
        let Ok(flags) = property32(connection, window, atoms.wm_state, AtomEnum::ATOM) else {
            continue;
        };
        if flags.contains(&atoms.hidden) {
            continue;
        }
        let desktop = property32(connection, window, atoms.wm_desktop, AtomEnum::CARDINAL)
            .ok()
            .and_then(|values| values.first().copied());
        let on_active_workspace = match (desktop, current_desktop) {
            (Some(desktop), Some(current)) => desktop == current || desktop == u32::MAX,
            _ => true,
        };
        if !on_active_workspace {
            continue;
        }
        let Ok(geometry_cookie) = connection.get_geometry(window) else {
            continue;
        };
        let Ok(geometry) = geometry_cookie.reply() else {
            continue;
        };
        // `get_geometry` is relative to the WM frame after reparenting. The
        // root coordinates are what monitor rectangles are expressed in.
        let origin = connection
            .translate_coordinates(window, root, 0, 0)
            .ok()
            .and_then(|cookie| cookie.reply().ok());
        let (x, y) = origin
            .map(|origin| (i32::from(origin.dst_x), i32::from(origin.dst_y)))
            .unwrap_or((i32::from(geometry.x), i32::from(geometry.y)));
        let width = i32::from(geometry.width);
        let height = i32::from(geometry.height);
        if width <= 0 || height <= 0 {
            continue;
        }
        windows.push(WindowRect {
            x,
            y,
            width,
            height,
            maximized: flags.contains(&atoms.maximized_vert)
                && flags.contains(&atoms.maximized_horz),
            fullscreen: flags.contains(&atoms.fullscreen),
        });
    }
    // Stacking order is not semantically relevant for coverage. Sorting keeps
    // a focus change from looking like a new window state and triggering an
    // Eco evaluation every 300 ms.
    windows.sort_by_key(|window| {
        (
            window.x,
            window.y,
            window.width,
            window.height,
            window.maximized,
            window.fullscreen,
        )
    });
    Ok(WindowState { windows })
}

fn property32(
    connection: &impl Connection,
    window: u32,
    property: u32,
    kind: AtomEnum,
) -> anyhow::Result<Vec<u32>> {
    let reply = connection
        .get_property(false, window, property, kind, 0, 4096)?
        .reply()?;
    Ok(reply
        .value32()
        .map(|values| values.collect())
        .unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, width: i32, height: i32) -> WindowRect {
        WindowRect {
            x,
            y,
            width,
            height,
            maximized: false,
            fullscreen: false,
        }
    }

    #[test]
    fn an_empty_window_list_covers_nothing() {
        let state = WindowState::default();
        assert_eq!(
            state.visibility_at(0, 0, 1920, 1080),
            MonitorVisibility::default()
        );
    }

    #[test]
    fn a_fullscreen_window_covers_only_its_own_monitor() {
        let state = WindowState {
            windows: vec![WindowRect {
                fullscreen: true,
                ..rect(1920, 0, 1920, 1080)
            }],
        };
        assert_eq!(
            state.visibility_at(0, 0, 1920, 1080),
            MonitorVisibility::default()
        );
        assert_eq!(
            state.visibility_at(1920, 0, 1920, 1080),
            MonitorVisibility {
                covered: true,
                fullscreen: true
            }
        );
    }

    #[test]
    fn a_window_spanning_both_monitors_covers_both() {
        let state = WindowState {
            windows: vec![WindowRect {
                fullscreen: true,
                ..rect(0, 0, 3840, 1080)
            }],
        };
        for x in [0, 1920] {
            let visibility = state.visibility_at(x, 0, 1920, 1080);
            assert!(visibility.covered, "monitor at {x} should be covered");
            assert!(visibility.fullscreen);
        }
    }

    #[test]
    fn a_small_window_does_not_pause_the_wallpaper() {
        let state = WindowState {
            windows: vec![rect(100, 100, 800, 600)],
        };
        assert!(!state.visibility_at(0, 0, 1920, 1080).covered);
    }

    #[test]
    fn a_maximized_window_with_a_panel_inset_still_counts() {
        // GNOME leaves the top bar outside a maximised window's client area,
        // so the rectangle is a few pixels short. The state flag and the
        // monitor centre are the reliable signal.
        let state = WindowState {
            windows: vec![WindowRect {
                maximized: true,
                ..rect(0, 32, 1920, 1048)
            }],
        };
        let visibility = state.visibility_at(0, 0, 1920, 1080);
        assert!(visibility.covered);
        assert!(!visibility.fullscreen);
    }

    #[test]
    fn workspace_filter_accepts_sticky_and_active() {
        fn on_active(desktop: Option<u32>, current: Option<u32>) -> bool {
            match (desktop, current) {
                (Some(desktop), Some(current)) => desktop == current || desktop == u32::MAX,
                _ => true,
            }
        }
        assert!(on_active(Some(1), Some(1)));
        assert!(on_active(Some(u32::MAX), Some(2)));
        assert!(!on_active(Some(0), Some(2)));
        assert!(on_active(None, Some(2)));
    }
}
