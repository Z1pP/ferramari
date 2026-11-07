//! One-shot probe: create a real desktop-layer wallpaper window on the live X
//! server, verify its EWMH properties, and destroy it.
//!
//! Run: `DISPLAY=:0 cargo run -p ferramari-media --example probe_surface`

use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::ConnectionExt as _;

fn root_monitor() -> anyhow::Result<ferramari_core::MonitorInfo> {
    let (connection, screen) = x11rb::connect(None)?;
    let setup = connection.setup();
    let info = &setup.roots[screen];
    Ok(ferramari_core::MonitorInfo {
        index: 0,
        name: "probe".to_string(),
        x: 0,
        y: 0,
        width: info.width_in_pixels as i32,
        height: info.height_in_pixels as i32,
        scale_factor: 1,
        is_primary: true,
    })
}

fn main() -> anyhow::Result<()> {
    assert!(ferramari_media::surface::available(), "no X server reachable");
    let monitor = root_monitor()?;

    let surface = ferramari_media::surface::create(&monitor)?;
    println!("created window {:#x} for {}", surface.xid, monitor.name);

    // Verify the window properties the WM will use to place it, over the
    // same connection that owns the window.
    let connection = surface.connection();
    let atom = |name: &str| -> anyhow::Result<u32> {
        Ok(connection.intern_atom(false, name.as_bytes())?.reply()?.atom)
    };
    let type_atom = atom("_NET_WM_WINDOW_TYPE")?;
    let desktop_atom = atom("_NET_WM_WINDOW_TYPE_DESKTOP")?;
    let wm_hints_atom = atom("WM_HINTS")?;
    let motif_atom = atom("_MOTIF_WM_HINTS")?;

    let types: Vec<u32> = connection
        .get_property(
            false,
            surface.xid,
            type_atom,
            x11rb::protocol::xproto::AtomEnum::ATOM,
            0,
            8,
        )?
        .reply()?
        .value32()
        .map(|v| v.collect())
        .unwrap_or_default();
    println!("_NET_WM_WINDOW_TYPE = {types:?} (desktop = {desktop_atom:#x})");
    assert!(
        types.contains(&desktop_atom),
        "window is not marked as DESKTOP type"
    );

    let hints: Vec<u32> = connection
        .get_property(
            false,
            surface.xid,
            wm_hints_atom,
            x11rb::protocol::xproto::AtomEnum::CARDINAL,
            0,
            9,
        )?
        .reply()?
        .value32()
        .map(|v| v.collect())
        .unwrap_or_default();
    println!("WM_HINTS = {hints:?}");
    let input = hints.get(8).copied().unwrap_or(1);
    assert_eq!(input, 0, "window should be non-focusable");

    let motif: Vec<u32> = connection
        .get_property(
            false,
            surface.xid,
            motif_atom,
            x11rb::protocol::xproto::AtomEnum::CARDINAL,
            0,
            5,
        )?
        .reply()?
        .value32()
        .map(|v| v.collect())
        .unwrap_or_default();
    println!("_MOTIF_WM_HINTS = {motif:?}");
    assert_eq!(motif.get(1).copied().unwrap_or(1), 0, "decorations should be off");

    let state = connection.get_window_attributes(surface.xid)?.reply()?;
    println!("map_state = {:?} (Viewable = mapped)", state.map_state);
    assert_eq!(
        state.map_state,
        x11rb::protocol::xproto::MapState::VIEWABLE,
        "window should be mapped"
    );

    // This is the id mpv receives as --wid.
    println!("mpv --wid value: {:#x}", surface.xid);

    drop(surface);
    println!("destroyed");
    Ok(())
}
