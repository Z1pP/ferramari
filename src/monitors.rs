//! Monitor discovery. Everything here runs on the GTK main thread because
//! `gdk::Monitor` is not `Send`.
//!
//! The monitor name doubles as the `data_source` key in the config, so it has
//! to survive reconnects and layout changes. GDK only exposes the EDID model,
//! which is not unique when two identical screens are attached, so duplicates
//! are disambiguated with the XRandR connector name (eDP-1, HDMI-1, …) instead
//! of the monitor index, which depends on enumeration order.

use std::collections::{HashMap, HashSet};

use gtk::gdk;
use gtk::prelude::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorInfo {
    pub index: i32,
    /// Model name, also used as the `data_source` key.
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub is_primary: bool,
}

fn display() -> Option<gdk::Display> {
    gdk::Display::default()
}

pub fn list() -> Vec<MonitorInfo> {
    let Some(display) = display() else {
        return Vec::new();
    };
    let connectors = xrandr_connectors();
    if !connectors.is_empty() {
        log::debug!("[Monitors] XRandR: {connectors:?}");
    }

    let mut raw = Vec::new();
    for index in 0..display.n_monitors() {
        let Some(monitor) = display.monitor(index) else {
            continue;
        };
        let base = model_name(&monitor, index);
        raw.push((monitor, index, base));
    }

    let mut counts: HashMap<String, usize> = HashMap::new();
    for (_, _, base) in &raw {
        *counts.entry(base.clone()).or_default() += 1;
    }

    let mut used = HashSet::new();
    let mut monitors = Vec::new();
    for (monitor, index, base) in raw {
        let connector = connector_for(&monitor, &connectors);
        let mut name = display_name(
            &base,
            counts.get(&base).copied().unwrap_or(1),
            connector.as_deref(),
            index,
        );
        if !used.insert(name.clone()) {
            // Belt and braces: never hand out two identical keys.
            name = format!("{base} #{index}");
            used.insert(name.clone());
        }
        let geometry = monitor.geometry();
        monitors.push(MonitorInfo {
            index,
            name,
            x: geometry.x(),
            y: geometry.y(),
            width: geometry.width(),
            height: geometry.height(),
            is_primary: monitor.is_primary(),
        });
    }
    if monitors.is_empty() {
        log::warn!("[Monitors] GDK reported no monitors");
    }
    monitors
}

/// `Monitor::model()` can be missing or duplicated on some drivers. Fall back
/// to the connector index.
fn model_name(monitor: &gdk::Monitor, index: i32) -> String {
    monitor
        .model()
        .map(|model| model.to_string())
        .filter(|model| !model.is_empty())
        .unwrap_or_else(|| format!("Monitor {index}"))
}

/// A unique model name is kept as-is, which keeps existing config keys
/// working. Only when several monitors share a model is the connector used:
/// it is stable across reboots and replugging, unlike the enumeration index.
fn display_name(base: &str, count: usize, connector: Option<&str>, index: i32) -> String {
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

/// Geometry and connector name for every RandR monitor. Empty when RandR is
/// unavailable or the extension is too old; naming then falls back to indexes.
fn xrandr_connectors() -> Vec<(i32, i32, i32, i32, String)> {
    use x11rb::connection::Connection as _;
    use x11rb::protocol::randr::ConnectionExt as _;
    use x11rb::protocol::xproto::ConnectionExt as _;

    let Ok((connection, screen)) = x11rb::connect(None) else {
        return Vec::new();
    };
    let root = connection.setup().roots[screen].root;
    let Ok(cookie) = connection.randr_get_monitors(root, true) else {
        return Vec::new();
    };
    let Ok(reply) = cookie.reply() else {
        return Vec::new();
    };

    reply
        .monitors
        .into_iter()
        .filter_map(|monitor| {
            let connector = monitor
                .outputs
                .first()
                .and_then(|output| {
                    let info = connection
                        .randr_get_output_info(*output, 0)
                        .ok()?
                        .reply()
                        .ok()?;
                    let name = String::from_utf8_lossy(&info.name).into_owned();
                    (!name.is_empty()).then_some(name)
                })
                .or_else(|| {
                    let reply = connection.get_atom_name(monitor.name).ok()?.reply().ok()?;
                    let name = String::from_utf8_lossy(&reply.name).into_owned();
                    (!name.is_empty()).then_some(name)
                })?;
            Some((
                i32::from(monitor.x),
                i32::from(monitor.y),
                i32::from(monitor.width),
                i32::from(monitor.height),
                connector,
            ))
        })
        .collect()
}

/// Match a GDK monitor to a RandR monitor by geometry. GDK reports logical
/// pixels while RandR reports device pixels, so the scaled rectangle is tried
/// too on mixed-DPI layouts.
fn connector_for(
    monitor: &gdk::Monitor,
    connectors: &[(i32, i32, i32, i32, String)],
) -> Option<String> {
    let geometry = monitor.geometry();
    let scale = monitor.scale_factor().max(1);
    let gdk = (
        geometry.x(),
        geometry.y(),
        geometry.width(),
        geometry.height(),
    );
    connectors.iter().find_map(|(x, y, width, height, name)| {
        rectangles_match(gdk, (*x, *y, *width, *height), scale).then(|| name.clone())
    })
}

/// GDK reports logical pixels while RandR reports device pixels. Accept both
/// rectangles, which keeps connector matching working on scaled outputs.
fn rectangles_match(gdk: (i32, i32, i32, i32), randr: (i32, i32, i32, i32), scale: i32) -> bool {
    gdk == randr || (gdk.0 * scale, gdk.1 * scale, gdk.2 * scale, gdk.3 * scale) == randr
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
        // if GDK enumerates them in a different order after a replug.
        let first = display_name("DELL U2412M", 2, Some("HDMI-1"), 0);
        let second = display_name("DELL U2412M", 2, Some("DP-1"), 1);
        assert_eq!(first, "DELL U2412M (HDMI-1)");
        assert_eq!(second, "DELL U2412M (DP-1)");
        assert_eq!(second, display_name("DELL U2412M", 2, Some("DP-1"), 0));
    }

    #[test]
    fn geometry_matching_accepts_logical_and_device_pixels() {
        let gdk = (0, 0, 1920, 1080);
        assert!(rectangles_match(gdk, (0, 0, 1920, 1080), 1));
        assert!(rectangles_match(gdk, (0, 0, 3840, 2160), 2));
        assert!(!rectangles_match(gdk, (0, 0, 2560, 1440), 1));
        // A different scale must not match another output by accident.
        assert!(!rectangles_match(gdk, (0, 0, 3840, 2160), 1));
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
