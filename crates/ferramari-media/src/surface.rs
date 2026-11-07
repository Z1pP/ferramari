//! Native X11 wallpaper surface.
//!
//! The GTK implementation created a `gtk::ApplicationWindow` per monitor,
//! downcast its GDK window to `gdkx11::X11Window` and handed the XID to mpv
//! via `--wid`. This module does the same with a raw X11 connection: one
//! toplevel per monitor, marked `_NET_WM_WINDOW_TYPE_DESKTOP` so the window
//! manager places it in the desktop layer, without decorations, and outside
//! the focus chain.
//!
//! The mpv embedding contract is unchanged: the caller receives the window id
//! and passes it as `--wid`.

use std::time::{Duration, Instant};

use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::{
    AtomEnum, ConfigureWindowAux, ConnectionExt as _, CreateWindowAux, EventMask, PropMode,
    StackMode, WindowClass,
};
use x11rb::rust_connection::RustConnection;

use ferramari_core::MonitorInfo;

/// A mapped wallpaper window. The owning connection is kept alive for the
/// lifetime of the surface; dropping it destroys the X window.
pub struct Surface {
    pub xid: u32,
    connection: RustConnection,
}

impl Surface {
    /// The connection this window was created on. Wallpaper windows only ever
    /// run on the main thread in this codebase, so the non-`Send` connection
    /// is not an issue; the X protocol is per-connection, so property queries
    /// must go through this one.
    pub fn connection(&self) -> &RustConnection {
        &self.connection
    }
}

/// Set a window property. `data` is the raw little-endian property payload
/// (32-bit atoms/cardinals or a UTF-8 string); `format` is 32 for atoms and
/// cardinals, 8 for strings.
fn set_prop(
    connection: &RustConnection,
    window: u32,
    property: u32,
    type_: u32,
    format: u8,
    data: &[u8],
) -> anyhow::Result<()> {
    // The X `length` is the number of items of the given `format` size.
    let item_size = (format / 8).max(1) as u32;
    let length = data.len() as u32 / item_size;
    connection
        .change_property(PropMode::REPLACE, window, property, type_, format, length, data)?;
    Ok(())
}

fn atom_bytes(values: &[u32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(values.len() * 4);
    for value in values {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

fn utf8_bytes(text: &str) -> Vec<u8> {
    text.as_bytes().to_vec()
}

/// Create and map a desktop-layer window for `monitor`.
///
/// Returns the window id once the server has marked it viewable (bounded
/// wait), mirroring the GTK path's `wait_until_viewable`: mpv must not start
/// before the map has actually happened.
pub fn create(monitor: &MonitorInfo) -> anyhow::Result<Surface> {
    let (connection, screen) = x11rb::connect(None)?;
    let setup = connection.setup();
    let root = setup.roots[screen].root;

    let atoms = Atoms::new(&connection)?;
    // Same depth and visual GDK used for the wallpaper windows: the screen's
    // default. mpv matches its child window to the parent's visual, so an
    // exotic ARGB visual here only risks a CreateWindow Match error.
    let screen_info = &setup.roots[screen];
    let (depth, visual) = (screen_info.root_depth, screen_info.root_visual);

    let window = connection
        .generate_id()
        .map_err(|err| anyhow::anyhow!("could not allocate a window id: {err:?}"))?;
    x11rb::protocol::xproto::create_window(
        &connection,
        depth,
        window,
        root,
        monitor.x as i16,
        monitor.y as i16,
        monitor.width as u16,
        monitor.height as u16,
        0,
        WindowClass::INPUT_OUTPUT,
        visual,
        &CreateWindowAux::new().event_mask(
            EventMask::EXPOSURE
                | EventMask::STRUCTURE_NOTIFY
                | EventMask::BUTTON_PRESS,
        ),
    )?
        .check()
        .map_err(|err| anyhow::anyhow!("could not create wallpaper window: {err}"))?;

    // Desktop-type window: the WM places it in the desktop layer and skips it
    // in the taskbar and pager. Motif hints strip the decorations explicitly
    // (GTK did the same with `set_decorated(false)`), and WM_HINTS takes the
    // window out of the focus chain (`accept_focus(false)`).
    set_desktop_type(&connection, &atoms, window)?;
    set_no_decorations(&connection, &atoms, window)?;
    set_window_name(&connection, &atoms, window, "Ferramari Wallpaper")?;
    set_no_focus(&connection, &atoms, window)?;

    connection.map_window(window)?;
    wait_until_viewable(&connection, window);

    // Desktop-icons extensions keep their own window at the bottom of the
    // DESKTOP layer; a freshly mapped wallpaper window sits above it and hides
    // the icons. Lower ourselves now and again after the extension has
    // settled (2.5 s, matching the GTK implementation).
    lower(&connection, window);
    lower_later(window, Duration::from_millis(2500));

    Ok(Surface {
        xid: window,
        connection,
    })
}

impl Drop for Surface {
    fn drop(&mut self) {
        let _ = self.connection.unmap_window(self.xid);
        let _ = self.connection.destroy_window(self.xid);
        let _ = self.connection.flush();
    }
}

/// Put the window at the bottom of its stack level, below the desktop-icon
/// extensions.
fn lower(connection: &RustConnection, window: u32) {
    let _ = connection.configure_window(
        window,
        &ConfigureWindowAux::new().stack_mode(StackMode::BELOW),
    );
    let _ = connection.flush();
}

/// Short-lived thread that lowers the window again after `delay`, once the
/// desktop-icons extension has had a moment to settle.
fn lower_later(window: u32, delay: Duration) {
    let _ = std::thread::Builder::new()
        .name("ferramari-surface-lower".into())
        .spawn(move || {
            std::thread::sleep(delay);
            if let Ok((connection, _)) = x11rb::connect(None) {
                lower(&connection, window);
            }
        });
}

/// Wait until the window manager has mapped `window`. `map_window` only queues
/// the request; the actual MapNotify is produced by the X server and, on a
/// managed toplevel, after the window manager has placed it. Spawning mpv in
/// between loses that race: it selects structure events on its parent and
/// treats any MapNotify as its own, skipping mapping the video child.
///
/// Bounded so a compositor that never maps the window cannot freeze startup;
/// the player is still started, and the supervisor will replace it if the
/// surface stays unusable.
fn wait_until_viewable(connection: &RustConnection, xid: u32) {
    use x11rb::protocol::xproto::MapState;

    const TIMEOUT: Duration = Duration::from_secs(2);
    const POLL: Duration = Duration::from_millis(5);

    let deadline = Instant::now() + TIMEOUT;
    while Instant::now() < deadline {
        match connection
            .get_window_attributes(xid)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
        {
            Some(attributes) if attributes.map_state == MapState::VIEWABLE => return,
            Some(_) => std::thread::sleep(POLL),
            // The window disappeared (monitor removed, shutdown): no point
            // waiting for the rest of the timeout.
            None => return,
        }
    }
    log::warn!("[Wallpaper] window {xid:#x} was not mapped before the player started");
}

struct Atoms {
    net_wm_window_type: u32,
    net_wm_window_type_desktop: u32,
    motif_wm_hints: u32,
    net_wm_name: u32,
    utf8_string: u32,
    wm_hints: u32,
}

impl Atoms {
    fn new(connection: &RustConnection) -> anyhow::Result<Self> {
        let atom = |name: &str| -> anyhow::Result<u32> {
            Ok(connection
                .intern_atom(false, name.as_bytes())?
                .reply()?
                .atom)
        };
        Ok(Self {
            net_wm_window_type: atom("_NET_WM_WINDOW_TYPE")?,
            net_wm_window_type_desktop: atom("_NET_WM_WINDOW_TYPE_DESKTOP")?,
            motif_wm_hints: atom("_MOTIF_WM_HINTS")?,
            net_wm_name: atom("_NET_WM_NAME")?,
            utf8_string: atom("UTF8_STRING")?,
            wm_hints: atom("WM_HINTS")?,
        })
    }
}

fn set_desktop_type(
    connection: &RustConnection,
    atoms: &Atoms,
    window: u32,
) -> anyhow::Result<()> {
    set_prop(
        connection,
        window,
        atoms.net_wm_window_type,
        AtomEnum::ATOM.into(),
        32,
        &atom_bytes(&[atoms.net_wm_window_type_desktop]),
    )
}

/// `_MOTIF_WM_HINTS`: flags=2 (decorations field present), decorations=0.
fn set_no_decorations(
    connection: &RustConnection,
    atoms: &Atoms,
    window: u32,
) -> anyhow::Result<()> {
    let values: [u32; 5] = [2, 0, 0, 0, 0];
    set_prop(
        connection,
        window,
        atoms.motif_wm_hints,
        AtomEnum::CARDINAL.into(),
        32,
        &atom_bytes(&values),
    )
}

fn set_window_name(
    connection: &RustConnection,
    atoms: &Atoms,
    window: u32,
    name: &str,
) -> anyhow::Result<()> {
    set_prop(
        connection,
        window,
        atoms.net_wm_name,
        atoms.utf8_string,
        8,
        &utf8_bytes(name),
    )
}

/// `WM_HINTS` with `flags = 2` (input field present) and `input = False`,
/// keeping the window out of the focus chain like the GTK window's
/// `accept_focus(false)` / `can_focus(false)`.
fn set_no_focus(
    connection: &RustConnection,
    atoms: &Atoms,
    window: u32,
) -> anyhow::Result<()> {
    let values: [u32; 9] = [0x2, 0, 0, 0, 0, 0, 0, 0, 0];
    set_prop(
        connection,
        window,
        atoms.wm_hints,
        AtomEnum::CARDINAL.into(),
        32,
        &atom_bytes(&values),
    )
}

/// Whether the X server can be reached at all.
pub fn available() -> bool {
    x11rb::connect(None).is_ok()
}

/// Create one surface per monitor, returning the surfaces in monitor order.
/// The surfaces must be kept alive: dropping a [`Surface`] destroys its X
/// window. Monitors that fail to map are skipped with a warning.
pub fn create_all(monitors: &[MonitorInfo]) -> Vec<(Surface, MonitorInfo)> {
    monitors
        .iter()
        .filter_map(|monitor| match create(monitor) {
            Ok(surface) => Some((surface, monitor.clone())),
            Err(err) => {
                log::warn!(
                    "[Wallpaper] could not create surface for {}: {err}",
                    monitor.name
                );
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn available_does_not_panic_without_display() {
        // Without a reachable X server the probe must return false, not panic.
        let _ = available();
    }
}
