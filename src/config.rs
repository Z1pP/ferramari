//! User configuration: `~/.config/ferramari/config.json`.
//!
//! The file format uses schema version 4 so existing settings keep working.
//! Loading is deliberately forgiving:
//! anything missing, unknown or broken falls back to the template, and schema
//! 3 files are migrated in place.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::paths;

pub const CONFIG_VERSION: u32 = 4;

pub const MODE_NULL: &str = "MODE_NULL";
pub const MODE_VIDEO: &str = "MODE_VIDEO";
pub const MODE_STREAM: &str = "MODE_STREAM";
pub const MODE_WEBPAGE: &str = "MODE_WEBPAGE";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    #[serde(rename = "MODE_NULL")]
    Null,
    #[serde(rename = "MODE_VIDEO")]
    Video,
    #[serde(rename = "MODE_STREAM")]
    Stream,
    #[serde(rename = "MODE_WEBPAGE")]
    Webpage,
    /// A value written by a newer version. Treated as `Null`.
    #[serde(other)]
    Unknown,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Null | Mode::Unknown => MODE_NULL,
            Mode::Video => MODE_VIDEO,
            Mode::Stream => MODE_STREAM,
            Mode::Webpage => MODE_WEBPAGE,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    pub version: u32,
    pub mode: Mode,
    pub data_source: HashMap<String, String>,
    #[serde(rename = "is_mute")]
    pub is_mute: bool,
    #[serde(rename = "audio_volume")]
    pub volume: i64,
    #[serde(rename = "is_static_wallpaper")]
    pub is_static_wallpaper: bool,
    #[serde(rename = "static_wallpaper_blur_radius")]
    pub blur_radius: i64,
    #[serde(rename = "is_pause_when_maximized")]
    pub is_pause_when_maximized: bool,
    #[serde(rename = "is_mute_when_maximized")]
    pub is_mute_when_maximized: bool,
    #[serde(rename = "fade_duration_sec")]
    pub fade_duration_sec: f64,
    #[serde(rename = "fade_interval")]
    pub fade_interval: f64,
    #[serde(rename = "is_show_systray")]
    pub is_show_systray: bool,
    #[serde(rename = "is_first_time")]
    pub is_first_time: bool,
}

impl Config {
    pub fn template(monitors: &[String]) -> Self {
        let mut data_source = HashMap::new();
        for monitor in monitors {
            data_source.insert(monitor.clone(), String::new());
        }
        data_source.insert("Default".to_string(), String::new());
        Self {
            version: CONFIG_VERSION,
            mode: Mode::Null,
            data_source,
            is_mute: false,
            volume: 50,
            is_static_wallpaper: true,
            blur_radius: 5,
            is_pause_when_maximized: true,
            is_mute_when_maximized: false,
            fade_duration_sec: 1.5,
            fade_interval: 0.1,
            is_show_systray: false,
            is_first_time: true,
        }
    }

    /// Load the config, creating or migrating it when needed.
    pub fn load(monitors: &[String]) -> Self {
        let path = paths::config_path();
        let Ok(text) = fs::read_to_string(&path) else {
            let template = Self::template(monitors);
            template.save();
            return template;
        };
        let (config, changed) = Self::parse(&text, monitors);
        if changed {
            config.save();
        }
        config
    }

    /// Parse a JSON document. Returns the config and whether the on-disk copy
    /// should be rewritten (migration, missing monitors, repaired source).
    pub fn parse(text: &str, monitors: &[String]) -> (Self, bool) {
        let template = Self::template(monitors);
        let template_value = serde_json::to_value(&template).expect("template is serializable");

        let mut value: Value = match serde_json::from_str(text) {
            Ok(value @ Value::Object(_)) => value,
            _ => return (template, true),
        };
        let mut changed = false;

        let version = value.get("version").and_then(Value::as_u64).unwrap_or(0);
        if version < 4 {
            if let Some(migrated) = migrate_v3_to_v4(&mut value) {
                changed |= migrated;
            }
        }

        // Keep only keys the current schema knows, then fill anything missing
        // from the template. This makes adding a setting a non-breaking change.
        let merged = merge(template_value, &value);
        let mut config: Config = match serde_json::from_value(merged) {
            Ok(config) => config,
            Err(err) => {
                log::warn!("[Config] Invalid config ({err}), using defaults");
                return (template, true);
            }
        };
        if config.version != CONFIG_VERSION {
            config.version = CONFIG_VERSION;
            changed = true;
        }

        // Monitors come and go; a new monitor inherits the Default source.
        for monitor in monitors {
            if !config.data_source.contains_key(monitor) {
                let default = config
                    .data_source
                    .get("Default")
                    .cloned()
                    .unwrap_or_default();
                config.data_source.insert(monitor.clone(), default);
                changed = true;
            }
        }

        // Repair a stale Default video source.
        if config.mode == Mode::Video {
            let default = config
                .data_source
                .get("Default")
                .cloned()
                .unwrap_or_default();
            if !Path::new(&default).is_file() {
                let replacement = config
                    .data_source
                    .values()
                    .find(|value| !value.is_empty() && Path::new(value.as_str()).is_file())
                    .cloned();
                if let Some(replacement) = replacement {
                    config
                        .data_source
                        .insert("Default".to_string(), replacement);
                    changed = true;
                }
            }
        }

        (config, changed)
    }

    pub fn save(&self) {
        let path = paths::config_path();
        if let Some(parent) = path.parent() {
            if let Err(err) = fs::create_dir_all(parent) {
                log::warn!("[Config] Could not create {}: {err}", parent.display());
                return;
            }
        }
        match serde_json::to_string_pretty(self) {
            Ok(text) => {
                if let Err(err) = fs::write(&path, format!("{text}\n")) {
                    log::warn!("[Config] Could not write {}: {err}", path.display());
                }
            }
            Err(err) => log::warn!("[Config] Could not serialize config: {err}"),
        }
    }

    pub fn reset(monitors: &[String]) {
        Self::template(monitors).save();
    }

    pub fn default_source(&self) -> &str {
        self.data_source
            .get("Default")
            .map(String::as_str)
            .unwrap_or("")
    }

    /// The video source for a monitor: its own entry, else Default.
    pub fn source_for(&self, monitor: &str) -> &str {
        match self.data_source.get(monitor) {
            Some(source) if !source.is_empty() => source,
            _ => self.default_source(),
        }
    }
}

fn merge(mut template: Value, parsed: &Value) -> Value {
    let (Some(template), Some(parsed)) = (template.as_object_mut(), parsed.as_object()) else {
        return template;
    };
    for (key, value) in parsed {
        if template.contains_key(key) {
            template.insert(key.clone(), value.clone());
        }
    }
    Value::Object(template.clone())
}

/// Schema 3 stored the source of *all* monitors in one string and called
/// the maximise setting `is_detect_maximized`.
fn migrate_v3_to_v4(value: &mut Value) -> Option<bool> {
    let object = value.as_object_mut()?;
    let mut changed = false;

    if let Some(old_source) = object.remove("data_source") {
        let default = old_source.as_str().unwrap_or_default().to_string();
        let mut data_source = Map::new();
        data_source.insert("Default".to_string(), Value::String(default));
        object.insert("data_source".to_string(), Value::Object(data_source));
        changed = true;
    }
    if let Some(old) = object.remove("is_detect_maximized") {
        object.insert("is_pause_when_maximized".to_string(), old);
        changed = true;
    }
    if !object.contains_key("is_mute_when_maximized") {
        object.insert("is_mute_when_maximized".to_string(), json!(false));
        changed = true;
    }
    object.insert("version".to_string(), json!(4));
    Some(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitors() -> Vec<String> {
        vec!["eDP-1".to_string(), "HDMI-1".to_string()]
    }

    #[test]
    fn template_contains_every_monitor_and_default() {
        let config = Config::template(&monitors());
        assert_eq!(config.version, 4);
        assert_eq!(config.mode, Mode::Null);
        assert!(config.data_source.contains_key("Default"));
        assert!(config.data_source.contains_key("eDP-1"));
        assert_eq!(config.volume, 50);
        assert!(config.is_static_wallpaper);
    }

    #[test]
    fn modes_roundtrip_through_json() {
        for (mode, text) in [
            (Mode::Null, "\"MODE_NULL\""),
            (Mode::Video, "\"MODE_VIDEO\""),
            (Mode::Stream, "\"MODE_STREAM\""),
            (Mode::Webpage, "\"MODE_WEBPAGE\""),
        ] {
            let encoded = serde_json::to_string(&mode).unwrap();
            assert_eq!(encoded, text);
            assert_eq!(serde_json::from_str::<Mode>(text).unwrap(), mode);
        }
        assert_eq!(Mode::Null.as_str(), MODE_NULL);
    }

    #[test]
    fn unknown_mode_does_not_lose_the_rest_of_the_config() {
        let text = r#"{
            "version": 4, "mode": "MODE_FROM_THE_FUTURE",
            "data_source": {"Default": "/videos/a.mp4"},
            "is_mute": true, "audio_volume": 21
        }"#;
        let (config, _) = Config::parse(text, &monitors());
        assert_eq!(config.mode, Mode::Unknown);
        assert!(config.is_mute);
        assert_eq!(config.volume, 21);
    }

    #[test]
    fn missing_keys_are_filled_from_the_template() {
        let text = r#"{"version": 4, "mode": "MODE_NULL", "data_source": {"Default": ""}}"#;
        let (config, changed) = Config::parse(text, &monitors());
        assert!(changed);
        assert_eq!(config.volume, 50);
        assert!(config.data_source.contains_key("HDMI-1"));
    }

    #[test]
    fn schema_three_is_migrated() {
        let text = r#"{
            "version": 3, "mode": "MODE_VIDEO",
            "data_source": "/videos/old.mp4",
            "is_detect_maximized": false,
            "audio_volume": 70
        }"#;
        let (config, changed) = Config::parse(text, &monitors());
        assert!(changed);
        assert_eq!(config.version, 4);
        assert_eq!(config.default_source(), "/videos/old.mp4");
        assert!(!config.is_pause_when_maximized);
        assert!(!config.is_mute_when_maximized);
        assert_eq!(config.volume, 70);
        assert!(config.data_source.contains_key("eDP-1"));
    }

    #[test]
    fn broken_json_falls_back_to_template() {
        let (config, changed) = Config::parse("not json", &monitors());
        assert!(changed);
        assert_eq!(config, Config::template(&monitors()));
    }

    #[test]
    fn invalid_video_source_is_repaired_when_a_valid_one_exists() {
        let valid =
            std::env::temp_dir().join(format!("ferramari-config-test-{}.mp4", std::process::id()));
        std::fs::write(&valid, b"not really a video").unwrap();
        let text = format!(
            r#"{{"version": 4, "mode": "MODE_VIDEO",
                 "data_source": {{"Default": "/does/not/exist.mp4", "eDP-1": "{}"}}}}"#,
            valid.display()
        );
        let (config, changed) = Config::parse(&text, &monitors());
        assert!(changed);
        assert_eq!(config.default_source(), valid.to_string_lossy());
        let _ = std::fs::remove_file(valid);
    }

    #[test]
    fn source_for_falls_back_to_default() {
        let mut config = Config::template(&monitors());
        config.data_source.insert("Default".into(), "/a.mp4".into());
        config.data_source.insert("eDP-1".into(), "/b.mp4".into());
        assert_eq!(config.source_for("eDP-1"), "/b.mp4");
        assert_eq!(config.source_for("HDMI-1"), "/a.mp4");
        assert_eq!(config.source_for("Unknown"), "/a.mp4");
    }
}
