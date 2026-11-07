//! End-to-end: the GTK-free playback Engine — platform-owned X11 surface + real mpv
//! in one object. Exercises play/pause/volume/mute/fps-cap against a live
//! player and asserts mpv reparented its output into our window.
//!
//! Run: `DISPLAY=:0 cargo run -p ferramari-platform --example probe_engine <video>`

use std::time::Duration;

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
    let video = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/home/blanc/Videos/Ferramari/arcane-jinx.mp4".to_string());
    let monitor = root_monitor()?;

    let mut engine = ferramari_media::engine::Engine::video(
        &ferramari_platform::surface::X11SurfaceProvider,
        &monitor,
        &video,
        60,
        false,
        "balanced",
        "auto",
    )?;
    println!("engine up on {:#x}", engine.surface_handle());

    // mpv must have reparented its output window into ours — the exact
    // contract the old GTK path relied on.
    std::thread::sleep(Duration::from_millis(1200));
    let alive = engine.player_alive();
    println!("player alive: {alive}");
    assert!(alive, "mpv did not start");

    // Inspect mpv's reparenting — windows are visible from *any* X
    // connection, so we open our own instead of borrowing the surface's.
    let (inspector, _) = x11rb::connect(None)?;
    let cookie = inspector.query_tree(engine.surface_handle() as u32)?;
    let reply = cookie.reply()?;
    let tree = reply.children.len();
    drop(inspector);
    println!("child windows: {tree}");
    assert!(tree >= 1, "mpv created no child inside the surface");

    // Drive the same control surface the application loop uses.
    engine.set_volume(75);
    engine.set_mute(true);
    println!("volume/mute: ok");
    engine.set_fps_cap(Some(30));
    println!("fps cap: ok");
    engine.set_fps_cap(None);

    // Pause with fade on its own slot, then resume.
    engine.pause_fade(0.2, 0.05);
    std::thread::sleep(Duration::from_millis(350));
    engine.play_fade(75, 0.2, 0.05);
    std::thread::sleep(Duration::from_millis(350));
    println!("fade pause/play: alive={}", engine.player_alive());

    // Park/unpark path, as Eco would use it: pause first, then release the
    // decoder once the hidden pause has lasted long enough. park_if_due
    // deliberately refuses before there has been a pause at all.
    engine.pause_fade(0.2, 0.05);
    std::thread::sleep(Duration::from_millis(400));
    engine.set_park_allowed(true);
    let too_early = engine.park_if_due(std::time::Instant::now(), Duration::from_secs(600));
    println!("park while pause is still short: {too_early} (expected false)");
    assert!(!too_early, "parked before the threshold had elapsed");

    let parked = engine.park_if_due(
        std::time::Instant::now() + Duration::from_secs(600),
        Duration::from_secs(1),
    );
    println!("park after a long hidden pause: {parked} (expected true)");
    assert!(parked, "a due park was refused");
    std::thread::sleep(Duration::from_millis(500));

    // A visible monitor must never show an empty surface: revoking parking
    // permission reloads the parked file.
    engine.set_park_allowed(false);
    std::thread::sleep(Duration::from_millis(800));
    println!(
        "after unpark: alive={} error={:?}",
        engine.player_alive(),
        engine.take_error()
    );
    assert!(engine.player_alive(), "player died across park/unpark");

    // Hot-swap the source, keeping the same surface and process.
    if let Some(second) = std::env::args().nth(2) {
        engine.load_video(&second, 60, false)?;
        std::thread::sleep(Duration::from_millis(800));
        println!("load_video -> {second}: alive={}", engine.player_alive());
    }

    let xid = engine.surface_handle() as u32;
    println!("dropping engine (kills mpv, unmaps window)");
    drop(engine);
    std::thread::sleep(Duration::from_millis(400));

    // The window must be gone: an over-eager second connection sees BadWindow
    // as a synchronous reply error, which does not poison the connection.
    let (connection, _) = x11rb::connect(None)?;
    let gone = connection
        .get_window_attributes(xid)
        .map_err(|e| anyhow::anyhow!("req: {e}"))?
        .reply()
        .is_err();
    assert!(gone, "window {xid:#x} survived the engine drop");
    println!("window {xid:#x} gone: ok");
    println!("done");
    Ok(())
}
