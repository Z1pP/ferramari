//! Translation from the public control vocabulary to [`Command`] values.
//!
//! Linux exposes the same vocabulary as typed D-Bus methods; Windows exposes
//! it as newline-delimited JSON. The tracked-request path on both transports
//! needs one implementation, so the mapping lives here where it can be tested
//! without a bus or a live desktop.

use std::path::PathBuf;

use serde_json::Value;

use crate::bus::Command;

fn string_param(params: &Value, index: usize) -> Result<String, String> {
    params
        .get("args")
        .and_then(Value::as_array)
        .and_then(|args| args.get(index))
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("missing string argument {index}"))
}

fn optional_string_param(params: &Value, index: usize) -> Option<String> {
    params
        .get("args")
        .and_then(Value::as_array)
        .and_then(|args| args.get(index))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// Map a command name plus `{ "args": [...] }` to the internal command.
pub fn command_from_json(method: &str, params: &Value) -> Result<Command, String> {
    let command = match method {
        "null" => Command::Null,
        "video" => Command::Video {
            path: optional_string_param(params, 0),
            monitor: optional_string_param(params, 1),
        },
        "stream" => Command::Stream {
            url: string_param(params, 0)?,
        },
        "webpage" => Command::Webpage {
            url: string_param(params, 0)?,
        },
        "import_package" => Command::ImportPackage(PathBuf::from(string_param(params, 0)?)),
        "export_package" => Command::ExportPackage {
            video: PathBuf::from(string_param(params, 0)?),
            parent: PathBuf::from(string_param(params, 1)?),
        },
        "sign_package" => Command::SignPackage(PathBuf::from(string_param(params, 0)?)),
        "verify_package" => Command::VerifyPackage(PathBuf::from(string_param(params, 0)?)),
        "trust_key" => Command::TrustKey(PathBuf::from(string_param(params, 0)?)),
        "library_changed" => Command::LibraryChanged,
        "pause_playback" => Command::PausePlayback,
        "start_playback" => Command::StartPlayback,
        "quit_player" => Command::QuitPlayer,
        "reload" => Command::Reload,
        "reload_config" => Command::ReloadConfig,
        "feeling_lucky" => Command::FeelingLucky,
        "show_gui" => Command::ShowGui,
        "quit" => Command::Quit,
        "toggle_mute" => Command::ToggleMute,
        "toggle_pause" => Command::TogglePause,
        "set_monitor_rotation" => Command::SetMonitorRotation {
            monitor: string_param(params, 0)?,
            collection: optional_string_param(params, 1),
        },
        other => return Err(format!("unknown server method {other:?}")),
    };
    Ok(command)
}

/// Map a property write to the internal command.
pub fn property_from_json(name: &str, value: &Value) -> Result<Command, String> {
    let command = match name {
        "volume" => Command::SetVolume(json_int(value)?),
        "blur_radius" => Command::SetBlurRadius(json_int(value)?),
        "is_mute" => Command::SetMute(json_bool(value)?),
        "is_paused_by_user" => Command::SetPausedByUser(json_bool(value)?),
        "is_static_wallpaper" => Command::SetStaticWallpaper(json_bool(value)?),
        "is_pause_when_maximized" => Command::SetPauseWhenMaximized(json_bool(value)?),
        "is_mute_when_maximized" => Command::SetMuteWhenMaximized(json_bool(value)?),
        "theme" => Command::SetTheme(json_string(value)?),
        "language" => Command::SetLanguage(json_string(value)?),
        "rotation_mode" => Command::SetRotationMode(json_string(value)?),
        "rotation_interval_sec" => Command::SetRotationInterval(json_int(value)?),
        "playback_profile" => Command::SetPlaybackProfile(json_string(value)?),
        "pause_on_battery" => Command::SetPauseOnBattery(json_bool(value)?),
        "idle_timeout_sec" => Command::SetIdleTimeout(json_int(value)?),
        "hwdec" => Command::SetHardwareDecoding(json_string(value)?),
        "web_allow_network" => Command::SetWebNetwork(json_bool(value)?),
        "is_show_systray" => Command::SetShowSystray(json_bool(value)?),
        "autostart_enabled" => Command::SetAutostart(json_bool(value)?),
        other => return Err(format!("unknown or read-only property {other:?}")),
    };
    Ok(command)
}

fn json_int(value: &Value) -> Result<i64, String> {
    value
        .as_i64()
        .or_else(|| value.as_f64().map(|number| number as i64))
        .ok_or_else(|| "expected an integer".to_string())
}

fn json_bool(value: &Value) -> Result<bool, String> {
    value
        .as_bool()
        .ok_or_else(|| "expected a boolean".to_string())
}

fn json_string(value: &Value) -> Result<String, String> {
    value
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| "expected a string".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn video_arguments_and_empty_defaults_are_parsed() {
        let command =
            command_from_json("video", &json!({ "args": ["C:/wall.mp4", "DISPLAY1"] })).unwrap();
        match command {
            Command::Video { path, monitor } => {
                assert_eq!(path.as_deref(), Some("C:/wall.mp4"));
                assert_eq!(monitor.as_deref(), Some("DISPLAY1"));
            }
            other => panic!("unexpected command: {other:?}"),
        }

        let command = command_from_json("video", &json!({ "args": ["", ""] })).unwrap();
        match command {
            Command::Video { path, monitor } => {
                assert!(path.is_none());
                assert!(monitor.is_none());
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn property_writes_map_to_the_expected_commands() {
        assert!(matches!(
            property_from_json("idle_timeout_sec", &json!(600)).unwrap(),
            Command::SetIdleTimeout(600)
        ));
        assert!(matches!(
            property_from_json("is_mute", &json!(true)).unwrap(),
            Command::SetMute(true)
        ));
        assert!(property_from_json("mode", &json!("MODE_VIDEO")).is_err());
    }
}
