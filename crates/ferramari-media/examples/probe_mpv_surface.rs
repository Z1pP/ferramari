//! End-to-end: a native X11 surface driven by mpv, with no GTK in the
//! process. Proves the video wallpaper pipeline works on the platform layer.
//!
//! Run: `DISPLAY=:0 cargo run -p ferramari-platform --example probe_mpv_surface <video>`
//!
//! Exits after a few seconds; the mpv child is terminated via its Drop impl,
//! which kills only the process this example spawned.

use std::time::Duration;

use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::ConnectionExt as _;

fn main() -> anyhow::Result<()> {
    let video = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/home/blanc/Videos/Ferramari/arcane-jinx.mp4".to_string());

    let monitors = ferramari_platform::monitors::list();
    let monitor = monitors
        .first()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("no monitors"))?;

    let surface = ferramari_platform::surface::create(&monitor)?;
    println!("surface {:#x} on {}", surface.xid, monitor.name);

    let mut mpv = ferramari_media::mpv::Mpv::spawn(&ferramari_media::mpv::Options {
        wid: Some(surface.xid as u64),
        source: video.clone(),
        audio_file: None,
        no_audio: true,
        volume: 0,
        muted: true,
        paused: false,
        loop_file: true,
        profile: "balanced".into(),
        hwdec: "auto".into(),
    })?;
    println!("mpv spawned for {video}");

    std::thread::sleep(Duration::from_secs(3));

    println!("mpv alive: {}", mpv.is_running());
    println!("mpv last error: {:?}", mpv.take_error());

    let geometry = surface.connection().get_geometry(surface.xid)?.reply()?;
    println!(
        "surface geometry during playback: {}x{} +{}+{}",
        geometry.width, geometry.height, geometry.x, geometry.y
    );

    // The player must have reparented its output window into the surface —
    // exactly what makes `--wid` embedding work.
    let tree = surface.connection().query_tree(surface.xid)?.reply()?;
    println!(
        "child windows of surface: {} total",
        tree.children.len()
    );
    assert!(
        !tree.children.is_empty(),
        "mpv did not create a child window inside the surface"
    );

    // Drop mpv first (kills only our child), then the surface.
    drop(mpv);
    println!("mpv dropped");
    std::thread::sleep(Duration::from_millis(300));
    drop(surface);
    println!("done");
    Ok(())
}
