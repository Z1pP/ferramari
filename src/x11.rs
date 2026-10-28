//! X11 window-state watcher.
//!
//! Upstream depended on libwnck. The same information is available straight
//! from EWMH properties, so this module polls `_NET_CLIENT_LIST` and friends
//! from a background thread. Polling keeps it robust: no event races, no
//! per-window signal bookkeeping, and windows disappearing mid-query is
//! harmless.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::Duration;

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt};

use crate::app::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WindowState {
    pub is_any_maximized: bool,
    pub is_any_fullscreen: bool,
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

/// Start polling. Returns `None` on Wayland (upstream disables the feature
/// there too) or when X11 is not reachable.
pub fn start(tx: Sender<Command>) -> Option<Watcher> {
    if crate::paths::is_wayland() {
        log::info!("[WindowHandler] Wayland session, window polling disabled");
        return None;
    }
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

fn run(tx: Sender<Command>, stop: Arc<AtomicBool>) -> anyhow::Result<()> {
    let (connection, screen) = x11rb::connect(None)?;
    let root = connection.setup().roots[screen].root;
    let atoms = Atoms::new(&connection)?;

    let mut previous = None;
    while !stop.load(Ordering::Relaxed) {
        let state = query(&connection, root, &atoms).unwrap_or_default();
        if previous != Some(state) {
            log::debug!("[WindowHandler] {state:?}");
            if tx.send(Command::WindowState(state)).is_err() {
                break;
            }
            previous = Some(state);
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    Ok(())
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

    let mut state = WindowState::default();
    for window in clients.into_iter().filter(|window| *window != 0) {
        let types = property32(connection, window, atoms.wm_window_type, AtomEnum::ATOM)?;
        if types.contains(&atoms.type_desktop) || types.contains(&atoms.type_dock) {
            continue;
        }
        let flags = property32(connection, window, atoms.wm_state, AtomEnum::ATOM)?;
        if flags.contains(&atoms.hidden) {
            continue;
        }
        let desktop = property32(connection, window, atoms.wm_desktop, AtomEnum::CARDINAL)?
            .first()
            .copied();
        let on_active_workspace = match (desktop, current_desktop) {
            (Some(desktop), Some(current)) => desktop == current || desktop == u32::MAX,
            _ => true,
        };
        if !on_active_workspace {
            continue;
        }
        if flags.contains(&atoms.maximized_vert) && flags.contains(&atoms.maximized_horz) {
            state.is_any_maximized = true;
        }
        if flags.contains(&atoms.fullscreen) {
            state.is_any_fullscreen = true;
        }
    }
    Ok(state)
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

    #[test]
    fn window_state_defaults_to_idle() {
        let state = WindowState::default();
        assert!(!state.is_any_maximized);
        assert!(!state.is_any_fullscreen);
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
