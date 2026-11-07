//! One-shot probe: prints what `monitors::list()` sees on the live X server.
//! Used to verify the GTK-free enumeration against the config keys.
//!
//! Run: `DISPLAY=:0 cargo run -p ferramari-platform --example probe_monitors`

fn main() {
    for monitor in ferramari_platform::monitors::list() {
        println!(
            "monitor #{index} name={name:?} {width}x{height}+{x}+{y} scale={scale} primary={primary}",
            index = monitor.index,
            name = monitor.name,
            width = monitor.width,
            height = monitor.height,
            x = monitor.x,
            y = monitor.y,
            scale = monitor.scale_factor,
            primary = monitor.is_primary,
        );
    }
    println!("background: {}", ferramari_platform::gnome_bg::background_diagnostic());
}
