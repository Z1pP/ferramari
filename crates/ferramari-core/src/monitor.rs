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
//!
//! Naming alone cannot make those keys durable: a replugged screen can come
//! back with a different label, and two identical screens swap names when the
//! backend reports them in the opposite order. The [`DisplayIdentity`] facts
//! each display carries are hashed into a stable id, and the display registry
//! remembers which config key belongs to which id so an assignment follows the
//! physical screen.

use sha2::Digest as _;

/// What a backend knows about the *physical* screen behind a display slot.
///
/// Every field comes from the enumeration call itself, so it is available
/// before any config key has been chosen. Native handles stay in the platform
/// adapter: core receives EDID bytes only to hash them, never to display.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DisplayIdentity {
    /// Connector port (`eDP-1`, `HDMI-1`, …) when the backend reports one.
    pub connector: Option<String>,
    /// Raw EDID of the attached screen; `None` when the backend cannot read
    /// it. A Wayland compositor hides EDID from its XWayland clients, so this
    /// is the common case under XWayland, not an error.
    pub edid: Option<Vec<u8>>,
    /// Physical screen size in millimetres, `0` when unknown. It separates two
    /// identical models plugged into different ports.
    pub width_mm: u32,
    pub height_mm: u32,
    /// Backend-native stable id (Win32 device path, macOS display UUID).
    /// Opaque to core: it only contributes to the identity hash.
    pub native_id: Option<String>,
}

impl DisplayIdentity {
    /// Identity of the *screen itself*, independent of which port it is
    /// plugged into: the EDID bytes, plus any backend-native serial. `None`
    /// when the backend exposes neither, which is the normal case under
    /// XWayland where the compositor hides EDID from X11 clients.
    pub fn screen_id(&self) -> Option<String> {
        let mut bytes = Vec::new();
        if let Some(edid) = &self.edid {
            bytes.extend_from_slice(edid);
        }
        if let Some(native_id) = &self.native_id {
            bytes.extend_from_slice(native_id.as_bytes());
        }
        if bytes.is_empty() {
            return None;
        }
        Some(digest_id(&bytes))
    }

    /// Identity that follows the *connector* instead of the panel. Used when
    /// screens cannot be told apart by EDID: replugging into the same port
    /// keeps the key, which is the best a port-pinned identity can promise.
    pub fn port_id(&self) -> Option<String> {
        let connector = self.connector.as_deref().filter(|name| !name.is_empty())?;
        let mut bytes = connector.as_bytes().to_vec();
        if self.width_mm != 0 && self.height_mm != 0 {
            bytes.extend_from_slice(&self.width_mm.to_le_bytes());
            bytes.extend_from_slice(&self.height_mm.to_le_bytes());
        }
        Some(digest_id(&bytes))
    }
}

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
    /// Facts that identify the physical screen, used to keep `name` stable.
    pub identity: DisplayIdentity,
}

impl MonitorInfo {
    /// Identity of the physical screen, falling back to the connector port.
    /// Resolution, position and scale are deliberately left out: they change
    /// when the user rearranges or rescales the layout, while a wallpaper
    /// assignment is supposed to follow the monitor itself.
    pub fn stable_id(&self) -> Option<String> {
        self.identity
            .screen_id()
            .or_else(|| self.identity.port_id())
    }
}

/// Short, stable identifier for a set of identity bytes: the first 16 hex
/// characters of their SHA-256 digest, prefixed so a config key can never
/// collide with a human-readable model name.
pub(crate) fn digest_id(bytes: &[u8]) -> String {
    let digest = sha2::Sha256::digest(bytes);
    let mut hex = String::with_capacity(32);
    for byte in &digest[..16] {
        hex.push_str(&format!("{byte:02x}"));
    }
    format!("id-{hex}")
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
pub fn geometry_matches(a: (i32, i32, i32, i32), b: (i32, i32, i32, i32), scale: i32) -> bool {
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
