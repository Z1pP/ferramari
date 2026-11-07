//! Linux X11/EWMH window-state backend.
//!
//! Native window-system observations are converted into the platform-neutral
//! window geometry model owned by ferramari-core.

pub use ferramari_core::window::{MonitorVisibility, WindowRect, WindowState};
use ferramari_core::{Command, CommandSender};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ChangeWindowAttributesAux, ConnectionExt, EventMask};
use x11rb::protocol::Event;

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
