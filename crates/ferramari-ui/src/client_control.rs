//! Client for the running Ferramari server on Windows.
//!
//! Linux uses the session bus; Windows has no freedesktop bus, so the server
//! publishes a loopback endpoint (port plus per-process token) in
//! `control-endpoint.json`. This module speaks the same command and property
//! vocabulary as the D-Bus client, one newline-delimited JSON request per
//! connection.
//!
//! The transport calls block on loopback I/O, so every request runs on
//! `spawn_blocking`: the Iced event loop must never wait on a socket, not even
//! a local one. A failed request produces an unreachable [`ServerState`], which
//! the shell uses to drop the handle and retry `connect` on a later tick.

use std::collections::HashMap;

use serde_json::Value;

use ferramari_core::util::tr;
use ferramari_platform::control;

pub use crate::client_state::ServerState;

/// Cloneable handle to the server's published control endpoint.
#[derive(Clone)]
pub struct Server {
    endpoint: control::Endpoint,
}

impl Server {
    pub async fn connect() -> Result<Self, String> {
        let endpoint = control::read_endpoint().ok_or_else(|| tr("Daemon offline"))?;
        if endpoint.protocol != control::PROTOCOL_VERSION {
            return Err(format!(
                "the server speaks control protocol {}, this control surface speaks {}",
                endpoint.protocol,
                control::PROTOCOL_VERSION
            ));
        }
        Ok(Self { endpoint })
    }

    /// PID owning the endpoint, or `None` when the server is not running.
    pub async fn pid(&self) -> Option<u32> {
        Some(self.endpoint.pid)
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        let endpoint = self.endpoint.clone();
        let method = method.to_string();
        tokio::task::spawn_blocking(move || control::call(&endpoint, &method, params))
            .await
            .map_err(|err| err.to_string())?
    }

    /// Fire-and-forget command, matching the D-Bus method names.
    pub async fn command(&self, method: &str) -> Result<(), String> {
        self.call("command", serde_json::json!({ "name": method }))
            .await
            .map(|_| ())
    }

    /// Command with positional arguments (`video(path, monitor)`, …). The
    /// D-Bus body arrives here as a serialized tuple; the control protocol
    /// wants a plain array.
    pub async fn command_args<B>(&self, method: &str, body: &B) -> Result<(), String>
    where
        B: serde::Serialize + Sync,
    {
        let value = serde_json::to_value(body).map_err(|err| err.to_string())?;
        let args = match value {
            Value::Array(items) => items,
            other => vec![other],
        };
        self.call(
            "command",
            serde_json::json!({ "name": method, "args": args }),
        )
        .await
        .map(|_| ())
    }

    /// Player commands share the server command vocabulary on Windows.
    pub async fn player_command_args<B>(&self, method: &str, body: &B) -> Result<(), String>
    where
        B: serde::Serialize + Sync,
    {
        self.command_args(method, body).await
    }

    /// Tracked command: the server replies only after the command loop has
    /// applied the command or recorded an application error.
    pub async fn request_command<B>(&self, method: &str, body: &B) -> Result<(), String>
    where
        B: serde::Serialize + Sync,
    {
        let value = serde_json::to_value(body).map_err(|err| err.to_string())?;
        let args = match value {
            Value::Array(items) => items,
            other => vec![other],
        };
        self.call(
            "request_command",
            serde_json::json!({ "name": method, "args": args }),
        )
        .await
        .map(|_| ())
    }

    pub async fn request_property(
        &self,
        property: &str,
        value: serde_json::Value,
    ) -> Result<(), String> {
        self.call(
            "request_property",
            serde_json::json!({ "name": property, "value": value }),
        )
        .await
        .map(|_| ())
    }

    pub async fn import_package(&self, source: String) -> Result<(), String> {
        self.request_command("import_package", &(source,)).await
    }

    pub async fn export_package(&self, video: String, parent: String) -> Result<(), String> {
        self.request_command("export_package", &(video, parent))
            .await
    }

    pub async fn sign_package(&self, directory: String) -> Result<(), String> {
        self.request_command("sign_package", &(directory,)).await
    }

    pub async fn verify_package(&self, directory: String) -> Result<(), String> {
        self.request_command("verify_package", &(directory,)).await
    }

    pub async fn trust_key(&self, source: String) -> Result<(), String> {
        self.request_command("trust_key", &(source,)).await
    }

    pub async fn set_monitor_rotation(
        &self,
        monitor: String,
        collection: String,
    ) -> Result<(), String> {
        self.request_command("set_monitor_rotation", &(monitor, collection))
            .await
    }

    async fn set_property(&self, property: &str, value: Value) -> Result<(), String> {
        self.call(
            "property_set",
            serde_json::json!({ "name": property, "value": value }),
        )
        .await
        .map(|_| ())
    }

    pub async fn set_i32(&self, property: &str, value: i32) -> Result<(), String> {
        self.set_property(property, serde_json::json!(value)).await
    }

    pub async fn set_bool(&self, property: &str, value: bool) -> Result<(), String> {
        self.set_property(property, serde_json::json!(value)).await
    }

    pub async fn set_string(&self, property: &str, value: String) -> Result<(), String> {
        self.set_property(property, serde_json::json!(value)).await
    }

    /// Read the merged state. A transport failure is reported as an
    /// unreachable state, not as an error: the shell treats it as a dropped
    /// connection and retries later.
    pub async fn snapshot(&self) -> ServerState {
        match self.call("snapshot", Value::Null).await {
            Ok(value) => state_from_json(&value),
            Err(_) => ServerState::default(),
        }
    }
}

fn state_from_json(value: &Value) -> ServerState {
    let player_stats = string_map(value, "player_stats");
    ServerState {
        reachable: true,
        mode: string(value, "mode"),
        volume: int(value, "volume"),
        blur_radius: int(value, "blur_radius"),
        is_mute: boolean(value, "is_mute"),
        is_playing: boolean(value, "is_playing"),
        is_decoding: boolean(value, "is_decoding"),
        is_paused_by_user: boolean(value, "is_paused_by_user"),
        is_static_wallpaper: boolean(value, "is_static_wallpaper"),
        is_pause_when_maximized: boolean(value, "is_pause_when_maximized"),
        is_mute_when_maximized: boolean(value, "is_mute_when_maximized"),
        theme: string(value, "theme"),
        data_source: string_map(value, "data_source"),
        monitor_states: string_map(value, "monitor_states"),
        monitor_resolutions: string_map(value, "monitor_resolutions"),
        monitor_refresh: int_map(value, "monitor_refresh"),
        primary_monitor: string(value, "primary_monitor"),
        rotations: string_map(value, "rotations"),
        server_pid: value
            .get("server_pid")
            .and_then(Value::as_u64)
            .map(|pid| pid as u32),
        language: string(value, "language"),
        rotation_mode: string(value, "rotation_mode"),
        rotation_interval_sec: int(value, "rotation_interval_sec"),
        playback_profile: string(value, "playback_profile"),
        pause_on_battery: boolean(value, "pause_on_battery"),
        idle_timeout_sec: int(value, "idle_timeout_sec"),
        hwdec: string(value, "hwdec"),
        hwdec_running: string(value, "hwdec_running"),
        player_stats: ferramari_core::stats::PlaybackStats::from_bus_map(&player_stats),
        web_allow_network: boolean(value, "web_allow_network"),
        autostart_enabled: boolean(value, "autostart_enabled"),
        is_show_systray: boolean(value, "is_show_systray"),
        operation_status: string(value, "operation_status"),
        operation_error: boolean(value, "operation_error"),
        operation_seq: value
            .get("operation_seq")
            .and_then(Value::as_u64)
            .unwrap_or(0),
    }
}

fn string(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn int(value: &Value, key: &str) -> i32 {
    value
        .get(key)
        .and_then(Value::as_i64)
        .map(|number| number as i32)
        .unwrap_or(0)
}

fn boolean(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn string_map(value: &Value, key: &str) -> HashMap<String, String> {
    value
        .get(key)
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .filter_map(|(name, entry)| {
                    entry
                        .as_str()
                        .map(|entry| (name.clone(), entry.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn int_map(value: &Value, key: &str) -> HashMap<String, u32> {
    value
        .get(key)
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .filter_map(|(name, entry)| {
                    entry
                        .as_u64()
                        .map(|entry| (name.clone(), entry.min(u64::from(u32::MAX)) as u32))
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full_snapshot() -> Value {
        serde_json::json!({
            "mode": "MODE_VIDEO",
            "volume": 42,
            "blur_radius": 12,
            "is_mute": true,
            "is_playing": true,
            "is_decoding": true,
            "is_paused_by_user": false,
            "is_static_wallpaper": false,
            "is_pause_when_maximized": true,
            "is_mute_when_maximized": false,
            "theme": "dark",
            "language": "ru",
            "rotation_mode": "random",
            "rotation_interval_sec": 600,
            "playback_profile": "eco",
            "pause_on_battery": true,
            "idle_timeout_sec": 900,
            "hwdec": "none",
            "hwdec_running": "no",
            "player_stats": {
                "fps": "59.94",
                "codec": "hevc",
                "width": "1920",
                "height": "1080",
            },
            "web_allow_network": true,
            "autostart_enabled": true,
            "is_show_systray": true,
            "operation_status": "Saved",
            "operation_error": false,
            "operation_seq": 7,
            "data_source": { "Default": "/videos/wall.mp4" },
            "monitor_states": { "DISPLAY1": "active" },
            "monitor_resolutions": { "DISPLAY1": "1920 × 1080" },
            "monitor_refresh": { "DISPLAY1": 60 },
            "primary_monitor": "DISPLAY1",
            "rotations": { "DISPLAY1": "Nature" },
            "server_pid": 4242,
        })
    }

    #[test]
    fn every_ui_field_is_read_from_the_snapshot() {
        let state = state_from_json(&full_snapshot());
        assert!(state.reachable);
        assert_eq!(state.mode, "MODE_VIDEO");
        assert_eq!(state.volume, 42);
        assert_eq!(state.blur_radius, 12);
        assert!(state.is_mute);
        assert!(state.is_playing);
        assert!(state.is_decoding);
        assert!(state.is_pause_when_maximized);
        assert_eq!(state.theme, "dark");
        assert_eq!(state.language, "ru");
        assert_eq!(state.rotation_mode, "random");
        assert_eq!(state.rotation_interval_sec, 600);
        assert_eq!(state.playback_profile, "eco");
        assert!(state.pause_on_battery);
        assert_eq!(state.idle_timeout_sec, 900);
        assert_eq!(state.hwdec, "none");
        assert_eq!(state.hwdec_running, "no");
        assert_eq!(state.data_source["Default"], "/videos/wall.mp4");
        assert_eq!(state.monitor_states["DISPLAY1"], "active");
        assert_eq!(state.monitor_resolutions["DISPLAY1"], "1920 × 1080");
        assert_eq!(state.monitor_refresh["DISPLAY1"], 60);
        assert_eq!(state.primary_monitor, "DISPLAY1");
        assert_eq!(state.rotations["DISPLAY1"], "Nature");
        assert_eq!(state.server_pid, Some(4242));
        assert!(state.autostart_enabled);
        assert!(state.is_show_systray);
        assert_eq!(state.operation_status, "Saved");
        assert_eq!(state.operation_seq, 7);
        assert_eq!(state.player_stats.codec, "hevc");
        assert_eq!(state.player_stats.width, Some(1920));
    }

    #[test]
    fn a_partial_snapshot_defaults_instead_of_failing() {
        let state = state_from_json(&serde_json::json!({ "mode": "MODE_NULL" }));
        assert!(state.reachable);
        assert_eq!(state.mode, "MODE_NULL");
        assert_eq!(state.volume, 0);
        assert!(state.data_source.is_empty());
        assert!(!state.player_stats.active);
    }
}
