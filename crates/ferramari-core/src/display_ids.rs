//! Persistent display identities.
//!
//! Config keys (`data_source`, `rotations`, per-monitor Eco state) are named
//! after the monitor, and the enumeration name is only as durable as the
//! backend's mood: a replugged screen can come back labelled differently, and
//! two identical panels can be reported in the opposite order, which silently
//! swaps their wallpapers.
//!
//! This registry remembers which config key belongs to which physical screen.
//! A screen is recognised by [`MonitorInfo::stable_id`], so an assignment
//! survives replugging, reboots and reordering. Legacy names are never renamed
//! on purpose: the first time a screen is seen it keeps the name the
//! enumeration gave it, which is what configs from 1.9.x already contain.
//!
//! Identity is intentionally limited to what the backend can actually report.
//! Under XWayland the compositor hides EDID from X11 clients, so the fallback
//! is the connector port plus the physical size: that pins a wallpaper to the
//! socket rather than to the panel, which is still better than following
//! enumeration order, and it is the honest limit of the session.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::config::{read_with_backup, write_atomic};
use crate::monitor::MonitorInfo;
use crate::paths;

/// One remembered screen: its stable identity and the config key it owns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplayRecord {
    /// Stable identity of the physical screen, or of the connector port when
    /// the screen itself cannot be identified. See [`MonitorInfo::stable_id`].
    pub stable_id: String,
    /// The `data_source` / `rotations` key this screen owns. It is what the
    /// user sees and what existing configs are indexed by.
    pub key: String,
}

/// Registry of screens this user has connected at least once.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplayIdRegistry {
    pub records: Vec<DisplayRecord>,
}

impl DisplayIdRegistry {
    /// Read the registry. A missing or unreadable file simply means "nothing
    /// remembered yet", which is the first-run case; the next bind rebuilds it
    /// from the live enumeration and legacy keys stay valid regardless.
    pub fn load() -> Self {
        Self::load_from(&paths::display_ids_path())
    }

    fn load_from(path: &Path) -> Self {
        let Some(text) = read_with_backup(path) else {
            return Self::default();
        };
        match serde_json::from_str(&text) {
            Ok(registry) => registry,
            Err(err) => {
                log::warn!("[DisplayIds] Ignoring {}: {err}", path.display());
                Self::default()
            }
        }
    }

    pub fn save(&self) -> Result<(), String> {
        self.save_to(&paths::display_ids_path())
    }

    fn save_to(&self, path: &Path) -> Result<(), String> {
        let text = serde_json::to_string_pretty(self)
            .map_err(|err| format!("could not serialize display ids: {err}"))?;
        write_atomic(path, format!("{text}\n").as_bytes())
            .map_err(|err| format!("could not write {}: {err}", path.display()))
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// The remembered key for a stable identity, if any.
    pub fn key_for(&self, stable_id: &str) -> Option<&str> {
        self.records
            .iter()
            .find(|record| record.stable_id == stable_id)
            .map(|record| record.key.as_str())
    }

    /// Give every display its durable config key and record the assignment.
    ///
    /// A screen already in the registry reclaims its key, so a wallpaper stays
    /// on the monitor it was configured for even when the backend reports a
    /// different model name or a different order. A screen seen for the first
    /// time keeps the enumeration name, which is exactly the key a pre-2.0
    /// config uses.
    ///
    /// Displays without a usable identity keep their enumeration name as-is:
    /// there is nothing to bind them to, and inventing a key would rewrite
    /// config entries the user can already see.
    ///
    /// Returns whether the registry changed, so the caller knows to persist it.
    ///
    /// Keys stay unique within one enumeration. A cloned layout reports two
    /// monitors for a single output, so both resolve to the same identity; the
    /// second one is recorded under a suffixed id and keeps its own key rather
    /// than sharing the first monitor's wallpaper assignment.
    pub fn bind(&mut self, monitors: &mut [MonitorInfo]) -> bool {
        let mut changed = false;
        let mut taken: Vec<String> = Vec::new();
        for monitor in monitors.iter_mut() {
            let Some(identity) = monitor.stable_id() else {
                continue;
            };
            // Distinguish repeat identities within this enumeration pass.
            let stable_id = match taken.iter().filter(|used| **used == identity).count() {
                0 => identity.clone(),
                repeats => format!("{identity}#{}", repeats + 1),
            };
            taken.push(identity);

            if let Some(key) = self.key_for(&stable_id) {
                // The key is the label as well: it is the name the screen was
                // first configured under, and it must not drift with the
                // wording of the backend.
                monitor.name = key.to_string();
                continue;
            }
            let key = self.free_key(&monitor.name, &stable_id);
            monitor.name = key.clone();
            self.records.push(DisplayRecord { stable_id, key });
            changed = true;
        }
        changed
    }

    /// A key for a newly seen screen: the name the backend reported whenever
    /// nothing owns it. The enumeration already disambiguates duplicate models
    /// by connector, so a collision means a remembered screen is away and its
    /// key must not be handed out to a different panel. Falling back to the
    /// identity hash keeps the two assignments separate.
    fn free_key(&self, desired: &str, stable_id: &str) -> String {
        if self.key_taken(desired) {
            stable_id.to_string()
        } else {
            desired.to_string()
        }
    }

    fn key_taken(&self, key: &str) -> bool {
        self.records.iter().any(|record| record.key == key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monitor::DisplayIdentity;

    fn port(name: &str, connector: &str, mm: u32) -> MonitorInfo {
        MonitorInfo {
            index: 0,
            name: name.to_string(),
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
            scale_factor: 1,
            is_primary: false,
            identity: DisplayIdentity {
                connector: Some(connector.to_string()),
                edid: None,
                width_mm: mm,
                height_mm: mm,
                native_id: None,
            },
        }
    }

    fn screen(name: &str, edid: &[u8]) -> MonitorInfo {
        let mut monitor = port(name, "HDMI-1", 519);
        monitor.identity.edid = Some(edid.to_vec());
        monitor
    }

    #[test]
    fn a_first_sighting_keeps_the_enumeration_name_as_its_key() {
        // Legacy configs must not be rewritten: the name X11 reports is what
        // `data_source` already contains.
        let mut registry = DisplayIdRegistry::default();
        let mut monitors = vec![port("eDP-1", "eDP-1", 340)];
        assert!(registry.bind(&mut monitors));
        assert_eq!(monitors[0].name, "eDP-1");
        assert_eq!(registry.records.len(), 1);
        assert_eq!(registry.records[0].key, "eDP-1");
    }

    #[test]
    fn binding_the_same_screen_twice_is_not_a_change() {
        let mut registry = DisplayIdRegistry::default();
        let mut monitors = vec![screen("DELL U2412M", b"edid-dell")];
        assert!(registry.bind(&mut monitors));
        let mut again = vec![screen("DELL U2412M", b"edid-dell")];
        assert!(!registry.bind(&mut again));
        assert_eq!(monitors, again);
    }

    #[test]
    fn a_replugged_panel_reclaims_its_config_key() {
        let mut registry = DisplayIdRegistry::default();
        let mut monitors = vec![screen("DELL U2412M", b"edid-dell")];
        registry.bind(&mut monitors);

        // Same panel, different port, and the backend now labels it
        // differently because the connector changed.
        let mut replugged = vec![screen("DELL U2412M (DP-1)", b"edid-dell")];
        replugged[0].identity.connector = Some("DP-1".to_string());
        registry.bind(&mut replugged);
        assert_eq!(replugged[0].name, "DELL U2412M");
        assert_eq!(registry.records.len(), 1, "one screen, one record");
    }

    #[test]
    fn two_identical_panels_on_different_ports_stay_distinct() {
        let mut registry = DisplayIdRegistry::default();
        let mut monitors = vec![
            port("DELL U2412M", "HDMI-1", 519),
            port("DELL U2412M", "DP-1", 519),
        ];
        registry.bind(&mut monitors);
        assert_eq!(monitors[0].name, "DELL U2412M");
        assert_ne!(
            monitors[1].name, monitors[0].name,
            "the second panel must not share the first one's key"
        );
        assert_eq!(registry.records.len(), 2);

        // Swapping the enumeration order must not swap the keys.
        let mut swapped = vec![
            port("DELL U2412M", "DP-1", 519),
            port("DELL U2412M", "HDMI-1", 519),
        ];
        registry.bind(&mut swapped);
        assert_eq!(swapped[0].name, monitors[1].name);
        assert_eq!(swapped[1].name, monitors[0].name);
    }

    #[test]
    fn resolution_and_position_are_not_part_of_the_identity() {
        let mut registry = DisplayIdRegistry::default();
        let mut monitors = vec![screen("eDP-1", b"edid-panel")];
        registry.bind(&mut monitors);

        let mut resized = vec![screen("eDP-1", b"edid-panel")];
        resized[0].width = 1280;
        resized[0].height = 720;
        resized[0].scale_factor = 2;
        resized[0].x = 1920;
        resized[0].index = 3;
        registry.bind(&mut resized);
        assert_eq!(resized[0].name, monitors[0].name);
        assert_eq!(registry.records.len(), 1);
    }

    #[test]
    fn an_unreadable_identity_leaves_the_name_alone() {
        // No EDID, no connector: there is nothing to bind, so the enumeration
        // name is used verbatim and nothing is remembered.
        let mut registry = DisplayIdRegistry::default();
        let mut monitors = vec![MonitorInfo {
            index: 0,
            name: "Monitor 1".to_string(),
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
            scale_factor: 1,
            is_primary: true,
            identity: DisplayIdentity::default(),
        }];
        assert!(!registry.bind(&mut monitors));
        assert_eq!(monitors[0].name, "Monitor 1");
        assert!(registry.is_empty());
    }

    #[test]
    fn a_new_screen_cannot_steal_a_key_that_is_already_taken() {
        let mut registry = DisplayIdRegistry::default();
        registry.bind(&mut [port("eDP-1", "eDP-1", 340)]);
        // A different panel on the same port: the port identity differs only
        // through the physical size, so this is a second screen.
        let mut monitors = vec![port("eDP-1", "eDP-1", 620)];
        registry.bind(&mut monitors);
        assert_ne!(monitors[0].name, "eDP-1");
        assert_eq!(registry.records.len(), 2);
    }

    #[test]
    fn a_renamed_screen_keeps_the_old_key_when_it_returns() {
        // An unplugged monitor keeps its record, so a screen that comes back
        // later reclaims the key instead of inheriting whatever the currently
        // attached monitor is called.
        let mut registry = DisplayIdRegistry::default();
        registry.bind(&mut [screen("HDMI TV", b"edid-tv")]);
        let mut monitors = vec![screen("HDMI-1", b"edid-tv")];
        registry.bind(&mut monitors);
        assert_eq!(monitors[0].name, "HDMI TV");
    }

    #[test]
    fn a_cloned_layout_keeps_both_legacy_names() {
        // Cloning reports one output as two monitors, so both carry the same
        // identity. The platform still labels them differently, and each keeps
        // its own key: sharing one key would give two windows one config entry.
        let mut registry = DisplayIdRegistry::default();
        let mut monitors = vec![
            screen("HDMI-1", b"edid-clone"),
            screen("HDMI-1 #1", b"edid-clone"),
        ];
        registry.bind(&mut monitors);
        assert_eq!(monitors[0].name, "HDMI-1");
        assert_eq!(monitors[1].name, "HDMI-1 #1");
        assert_eq!(registry.records.len(), 2);
        assert_ne!(registry.records[0].stable_id, registry.records[1].stable_id);
    }

    #[test]
    fn two_monitors_with_one_identity_and_one_label_are_still_separated() {
        // The pathological case a hand-rolled backend could report: the same
        // identity twice under the same name. Neither may take the other's key.
        let mut registry = DisplayIdRegistry::default();
        let mut monitors = vec![
            screen("eDP-1", b"edid-clone"),
            screen("eDP-1", b"edid-clone"),
        ];
        registry.bind(&mut monitors);
        assert_eq!(monitors[0].name, "eDP-1");
        assert_ne!(monitors[1].name, "eDP-1", "the pair needs distinct keys");
        assert_eq!(registry.records.len(), 2);
    }

    #[test]
    fn the_registry_survives_a_save_and_reload() {
        // The whole point of the file is that a restart re-attaches the same
        // keys, so a replug after a reboot cannot move a wallpaper.
        use std::fs;
        let dir = std::env::temp_dir().join(format!(
            "ferramari-display-ids-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let file = dir.join("display-ids.json");
        let mut registry = DisplayIdRegistry::default();
        let mut monitors = vec![screen("DELL U2412M", b"edid-dell")];
        registry.bind(&mut monitors);
        registry.save_to(&file).unwrap();

        let mut reloaded = DisplayIdRegistry::load_from(&file);
        assert_eq!(reloaded, registry);

        // The same panel, reported under a different name after a reboot:
        // the reloaded registry still hands back the original key.
        let mut after_reboot = vec![screen("eDP-1", b"edid-dell")];
        reloaded.bind(&mut after_reboot);
        assert_eq!(after_reboot[0].name, "DELL U2412M");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_corrupt_registry_file_is_ignored_not_fatal() {
        // A half-written file must not take the wallpapers down: the fallback
        // is the legacy behaviour of using the enumeration name.
        use std::fs;
        let dir =
            std::env::temp_dir().join(format!("ferramari-display-ids-bad-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("display-ids.json");
        fs::write(&file, "{ not json").unwrap();
        let registry = DisplayIdRegistry::load_from(&file);
        assert!(registry.is_empty());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn records_round_trip_through_json() {
        let mut registry = DisplayIdRegistry::default();
        registry.bind(&mut [screen("eDP-1", b"edid-a")]);
        let text = serde_json::to_string(&registry).unwrap();
        let parsed: DisplayIdRegistry = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed, registry);
    }

    #[test]
    fn an_empty_registry_binds_nothing() {
        let mut registry = DisplayIdRegistry::default();
        assert!(!registry.bind(&mut []));
        assert!(registry.is_empty());
    }
}
