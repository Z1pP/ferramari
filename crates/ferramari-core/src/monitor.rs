//! Monitor model and naming rules, toolkit-independent.
//!
//! The actual enumeration (currently X11 XRandR, previously GDK) lives in
//! `ferramari-platform::monitors`. This module owns the data shape and the
//! naming/disambiguation rules, which the config keys depend on: the monitor
//! `name` doubles as the `data_source` key, so it has to survive reconnects
//! and layout changes.
//!
//! The monitor name is the EDID model when it is unique among connected
//! displays. When several identical screens are attached, the connector name
//! (eDP-1, HDMI-1, …) is used instead of the enumeration index: the connector
//! is stable across reboots and replugging, the index is not.

/// One connected display, in logical pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorInfo {
    pub index: i32,
    /// Model name, also used as the `data_source` key.
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    /// Logical-to-device-pixel factor. Root-window coordinates (EWMH window
    /// geometry) are in device pixels, so occlusion checks scale by this.
    pub scale_factor: i32,
    pub is_primary: bool,
}

/// A unique model name is kept as-is, which keeps existing config keys
/// working. Only when several monitors share a model is the connector used:
/// it is stable across reboots and replugging, unlike the enumeration index.
pub fn display_name(base: &str, count: usize, connector: Option<&str>, index: i32) -> String {
    if count <= 1 {
        return base.to_string();
    }
    match connector {
        Some(connector) if !connector.is_empty() && connector != base => {
            format!("{base} ({connector})")
        }
        _ => format!("{base} #{index}"),
    }
}

/// A geometry a backend reports. Kept separate from the backend-specific
/// types so the matching rule stays testable without an X connection.
#[derive(Debug, Clone, Copy)]
pub struct Geometry {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// Enumerators report in different pixel units depending on the backend
/// (logical vs device pixels). Accept both rectangles, which keeps connector
/// matching working on scaled outputs.
pub fn geometry_matches(
    a: (i32, i32, i32, i32),
    b: (i32, i32, i32, i32),
    scale: i32,
) -> bool {
    a == b || (a.0 * scale, a.1 * scale, a.2 * scale, a.3 * scale) == b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_unique_model_keeps_its_short_key() {
        assert_eq!(display_name("eDP-1", 1, Some("eDP-1"), 0), "eDP-1");
        assert_eq!(
            display_name("DELL U2412M", 1, Some("HDMI-1"), 0),
            "DELL U2412M"
        );
    }

    #[test]
    fn duplicate_models_use_the_connector_not_the_index() {
        // The two physically identical screens get stable, distinct keys even
        // if the enumerator reports them in a different order after a replug.
        let first = display_name("DELL U2412M", 2, Some("HDMI-1"), 0);
        let second = display_name("DELL U2412M", 2, Some("DP-1"), 1);
        assert_eq!(first, "DELL U2412M (HDMI-1)");
        assert_eq!(second, "DELL U2412M (DP-1)");
        assert_eq!(second, display_name("DELL U2412M", 2, Some("DP-1"), 0));
    }

    #[test]
    fn geometry_matching_accepts_logical_and_device_pixels() {
        let gdk = (0, 0, 1920, 1080);
        assert!(geometry_matches(gdk, (0, 0, 1920, 1080), 1));
        assert!(geometry_matches(gdk, (0, 0, 3840, 2160), 2));
        assert!(!geometry_matches(gdk, (0, 0, 2560, 1440), 1));
        // A different scale must not match another output by accident.
        assert!(!geometry_matches(gdk, (0, 0, 3840, 2160), 1));
    }

    #[test]
    fn missing_connector_falls_back_to_the_index() {
        assert_eq!(display_name("Monitor 1", 2, None, 1), "Monitor 1 #1");
        assert_eq!(
            display_name("DELL U2412M", 2, Some("DELL U2412M"), 0),
            "DELL U2412M #0"
        );
    }
}
