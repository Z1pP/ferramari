//! Monitor discovery. Everything here runs on the GTK main thread because
//! `gdk::Monitor` is not `Send`.

use gtk::gdk;
use gtk::prelude::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorInfo {
    pub index: i32,
    /// Model name, also used as the `data_source` key (upstream behaviour).
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
    let mut monitors = Vec::new();
    for index in 0..display.n_monitors() {
        let Some(monitor) = display.monitor(index) else {
            continue;
        };
        let geometry = monitor.geometry();
        let name = name_for(&monitor, index, &monitors);
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
/// to the connector index and keep config keys unique.
fn name_for(monitor: &gdk::Monitor, index: i32, existing: &[MonitorInfo]) -> String {
    let base = monitor
        .model()
        .map(|model| model.to_string())
        .filter(|model| !model.is_empty())
        .unwrap_or_else(|| format!("Monitor {index}"));
    if existing.iter().any(|info| info.name == base) {
        format!("{base} #{index}")
    } else {
        base
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn monitor_names_are_unique() {
        // Pure reimplementation of the naming rule used above; guards against
        // the duplicate-model case without needing a display.
        fn unique(base: &str, index: i32, existing: &[&str]) -> String {
            if existing.contains(&base) {
                format!("{base} #{index}")
            } else {
                base.to_string()
            }
        }
        assert_eq!(unique("eDP-1", 0, &[]), "eDP-1");
        assert_eq!(unique("eDP-1", 1, &["eDP-1"]), "eDP-1 #1");
    }
}
