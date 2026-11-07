//! One-shot probe: show what the live display enumeration can identify, and
//! what config key the registry hands out for it.
//!
//! This is the runtime check for stable display IDs: the keys must match what
//! an existing `config.json` already contains, and the identity fields must
//! show what the current session can actually prove (EDID is hidden from
//! XWayland clients, so a port identity is the expected result there).
//!
//! Run: `DISPLAY=:0 cargo run -p ferramari-platform --example probe_display_ids`

use ferramari_core::display_ids::DisplayIdRegistry;

fn main() {
    let mut monitors = ferramari_platform::monitors::list();
    let mut registry = DisplayIdRegistry::default();
    registry.bind(&mut monitors);

    for monitor in &monitors {
        println!(
            "#{index} key={key:?} name={name:?} {width}x{height}+{x}+{y} scale={scale} primary={primary}",
            index = monitor.index,
            key = monitor.name,
            name = monitor.name,
            width = monitor.width,
            height = monitor.height,
            x = monitor.x,
            y = monitor.y,
            scale = monitor.scale_factor,
            primary = monitor.is_primary,
        );
        println!(
            "    connector={:?} edid={} bytes mm={}x{} stable_id={:?}",
            monitor.identity.connector,
            monitor.identity.edid.as_ref().map_or(0, Vec::len),
            monitor.identity.width_mm,
            monitor.identity.height_mm,
            monitor
                .identity
                .screen_id()
                .or_else(|| monitor.identity.port_id()),
        );
    }
    println!("registry: {:?}", registry.records);

    // Re-enumerating and rebinding must be idempotent: the same session twice
    // cannot invent a second key for a screen it already knows.
    let mut again = ferramari_platform::monitors::list();
    let changed = registry.bind(&mut again);
    println!("second bind changed the registry: {changed}");
    for (before, after) in monitors.iter().zip(&again) {
        assert_eq!(before.name, after.name, "keys must not drift");
    }
}
