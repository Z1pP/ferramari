//! D-Bus client for the running Ferramari server.
//!
//! The server exports two interfaces:
//! * `.server` — GUI-facing commands and the state properties,
//! * `.player` — playback commands plus `data_source` / `monitor_states`.
//!
//! The server does **not** emit `PropertiesChanged` (its snapshot is
//! refreshed in-process under a mutex), so the UI polls `GetAll` on both
//! interfaces and merges them into a [`ServerState`]. Commands are
//! fire-and-forget method calls; the next poll reflects the result. This is
//! the same contract existing scripts already use, so the UI is just another
//! bus client rather than a privileged one.

use std::collections::HashMap;

use zbus::Connection;
use zvariant::{OwnedValue, Value};

pub const SERVER_NAME: &str = "io.github.ferramari.Ferramari.server";
pub const SERVER_PATH: &str = "/io/github/ferramari/ferramari/server";
pub const PLAYER_PATH: &str = "/io/github/ferramari/ferramari/player";
pub const SERVER_INTERFACE: &str = "io.github.ferramari.ferramari.server";
pub const PLAYER_INTERFACE: &str = "io.github.ferramari.ferramari.player";
const PROPERTIES_INTERFACE: &str = "org.freedesktop.DBus.Properties";
const DBUS_INTERFACE: &str = "org.freedesktop.DBus";
const DBUS_PATH: &str = "/org/freedesktop/DBus";

/// Merged snapshot of both server interfaces.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ServerState {
    /// False until a reply arrives: the UI shows a disconnected banner.
    pub reachable: bool,
    /// `MODE_NULL`, `MODE_VIDEO`, `MODE_STREAM` or `MODE_WEBPAGE`.
    pub mode: String,
    pub volume: i32,
    pub blur_radius: i32,
    pub is_mute: bool,
    pub is_playing: bool,
    pub is_paused_by_user: bool,
    pub is_static_wallpaper: bool,
    pub is_pause_when_maximized: bool,
    pub is_mute_when_maximized: bool,
    /// `system`, `light` or `dark`.
    pub theme: String,
    /// Monitor name (and `Default`) → wallpaper source.
    pub data_source: HashMap<String, String>,
    /// Monitor name → Eco state (`active`, `idle`, `occluded`, `suspended`).
    pub monitor_states: HashMap<String, String>,
    /// PID of the server process, for metrics of the real workload.
    pub server_pid: Option<u32>,

    // Settings mirrored from the config file.
    /// `system`, `en`, `ru` or `ja`.
    pub language: String,
    /// `off`, `order` or `random`.
    pub rotation_mode: String,
    pub rotation_interval_sec: i32,
    /// `quality`, `balanced` or `eco`.
    pub playback_profile: String,
    pub pause_on_battery: bool,
    pub idle_timeout_sec: i32,
    /// `auto`, `vaapi` or `none`.
    pub hwdec: String,
    pub web_allow_network: bool,
    pub autostart_enabled: bool,
}

/// Cloneable handle to the session bus, addressed at the Ferramari server.
#[derive(Clone)]
pub struct Server {
    connection: Connection,
}

impl Server {
    pub async fn connect() -> Result<Self, String> {
        let connection = Connection::session().await.map_err(|e| e.to_string())?;
        Ok(Self { connection })
    }

    /// PID owning the server name, or `None` when the server is not running.
    pub async fn pid(&self) -> Option<u32> {
        self.connection
            .call_method(
                Some(DBUS_INTERFACE),
                DBUS_PATH,
                Some(DBUS_INTERFACE),
                "GetConnectionUnixProcessID",
                &(SERVER_NAME,),
            )
            .await
            .ok()?
            .body()
            .deserialize::<u32>()
            .ok()
            .filter(|pid| *pid != 0)
    }

    /// Fire-and-forget method call on the server interface.
    pub async fn command(&self, method: &str) -> Result<(), String> {
        self.connection
            .call_method(
                Some(SERVER_NAME),
                SERVER_PATH,
                Some(SERVER_INTERFACE),
                method,
                &(),
            )
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// Method call with an argument tuple: `video(path, monitor)`,
    /// `stream(url)`, `webpage(url)`.
    pub async fn command_args<B>(&self, method: &str, body: &B) -> Result<(), String>
    where
        B: serde::Serialize + zvariant::DynamicType + Sync,
    {
        self.connection
            .call_method(
                Some(SERVER_NAME),
                SERVER_PATH,
                Some(SERVER_INTERFACE),
                method,
                body,
            )
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    pub async fn import_package(&self, source: String) -> Result<(), String> {
        self.command_args("import_package", &(source,)).await
    }

    pub async fn export_package(&self, video: String, parent: String) -> Result<(), String> {
        self.command_args("export_package", &(video, parent)).await
    }

    pub async fn sign_package(&self, directory: String) -> Result<(), String> {
        self.command_args("sign_package", &(directory,)).await
    }

    pub async fn verify_package(&self, directory: String) -> Result<(), String> {
        self.command_args("verify_package", &(directory,)).await
    }

    pub async fn trust_key(&self, source: String) -> Result<(), String> {
        self.command_args("trust_key", &(source,)).await
    }

    /// Write one server property through the standard Properties interface.
    pub async fn set_property(&self, property: &str, value: Value<'_>) -> Result<(), String> {
        let owned = OwnedValue::try_from(value).map_err(|e| e.to_string())?;
        self.connection
            .call_method(
                Some(SERVER_NAME),
                SERVER_PATH,
                Some(PROPERTIES_INTERFACE),
                "Set",
                &(SERVER_INTERFACE, property, owned),
            )
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// Typed property writers, so callers never spell out `OwnedValue`.
    pub async fn set_i32(&self, property: &str, value: i32) -> Result<(), String> {
        self.set_property(property, Value::from(value)).await
    }

    pub async fn set_bool(&self, property: &str, value: bool) -> Result<(), String> {
        self.set_property(property, Value::from(value)).await
    }

    pub async fn set_string(&self, property: &str, value: String) -> Result<(), String> {
        self.set_property(property, Value::from(value)).await
    }

    /// Read and merge both interfaces. Always returns a state; `reachable`
    /// tells the UI whether it came from a live server.
    pub async fn snapshot(&self) -> ServerState {
        let server_pid = self.pid().await;
        let mut state = ServerState {
            reachable: server_pid.is_some(),
            server_pid,
            ..ServerState::default()
        };
        if !state.reachable {
            return state;
        }

        if let Some(values) = self.get_all(SERVER_PATH, SERVER_INTERFACE).await {
            state.mode = string_of(&values, "mode");
            state.volume = int_of(&values, "volume");
            state.blur_radius = int_of(&values, "blur_radius");
            state.is_mute = bool_of(&values, "is_mute");
            state.is_playing = bool_of(&values, "is_playing");
            state.is_paused_by_user = bool_of(&values, "is_paused_by_user");
            state.is_static_wallpaper = bool_of(&values, "is_static_wallpaper");
            state.is_pause_when_maximized = bool_of(&values, "is_pause_when_maximized");
            state.is_mute_when_maximized = bool_of(&values, "is_mute_when_maximized");
            state.theme = string_of(&values, "theme");
            state.language = string_of(&values, "language");
            state.rotation_mode = string_of(&values, "rotation_mode");
            state.rotation_interval_sec = int_of(&values, "rotation_interval_sec");
            state.playback_profile = string_of(&values, "playback_profile");
            state.pause_on_battery = bool_of(&values, "pause_on_battery");
            state.idle_timeout_sec = int_of(&values, "idle_timeout_sec");
            state.hwdec = string_of(&values, "hwdec");
            state.web_allow_network = bool_of(&values, "web_allow_network");
            state.autostart_enabled = bool_of(&values, "autostart_enabled");
        }
        if let Some(values) = self.get_all(PLAYER_PATH, PLAYER_INTERFACE).await {
            state.data_source = string_map_of(&values, "data_source");
            state.monitor_states = string_map_of(&values, "monitor_states");
        }
        state
    }

    async fn get_all(
        &self,
        path: &str,
        interface: &str,
    ) -> Option<HashMap<String, OwnedValue>> {
        self.connection
            .call_method(
                Some(SERVER_NAME),
                path,
                Some(PROPERTIES_INTERFACE),
                "GetAll",
                &(interface,),
            )
            .await
            .ok()?
            .body()
            .deserialize::<HashMap<String, OwnedValue>>()
            .ok()
    }
}

fn string_of(values: &HashMap<String, OwnedValue>, key: &str) -> String {
    values
        .get(key)
        .and_then(|value| value.try_clone().ok())
        .and_then(|value| String::try_from(value).ok())
        .unwrap_or_default()
}

fn int_of(values: &HashMap<String, OwnedValue>, key: &str) -> i32 {
    values
        .get(key)
        .and_then(|value| i32::try_from(value).ok())
        .unwrap_or(0)
}

fn bool_of(values: &HashMap<String, OwnedValue>, key: &str) -> bool {
    values
        .get(key)
        .and_then(|value| bool::try_from(value).ok())
        .unwrap_or(false)
}

fn string_map_of(values: &HashMap<String, OwnedValue>, key: &str) -> HashMap<String, String> {
    values
        .get(key)
        .and_then(|value| value.try_clone().ok())
        .and_then(|value| HashMap::<String, String>::try_from(value).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_properties_default_instead_of_failing() {
        let values = HashMap::new();
        assert_eq!(string_of(&values, "mode"), "");
        assert_eq!(int_of(&values, "volume"), 0);
        assert!(!bool_of(&values, "is_mute"));
        assert!(string_map_of(&values, "data_source").is_empty());
    }

    #[test]
    fn values_round_trip_through_the_converters() {
        let mut values = HashMap::new();
        values.insert("mode".to_string(), owned(Value::from("MODE_VIDEO")));
        values.insert("volume".to_string(), owned(Value::from(77i32)));
        values.insert("is_mute".to_string(), owned(Value::from(true)));
        assert_eq!(string_of(&values, "mode"), "MODE_VIDEO");
        assert_eq!(int_of(&values, "volume"), 77);
        assert!(bool_of(&values, "is_mute"));
    }

    fn owned(value: Value<'static>) -> OwnedValue {
        OwnedValue::try_from(value).expect("owned value")
    }
}
