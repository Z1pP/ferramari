//! The transport-neutral snapshot the control surface renders.
//!
//! Linux receives this shape as two D-Bus `GetAll` replies; Windows receives
//! one JSON object over the loopback control channel. Keeping the struct in a
//! transport-free module means every page reads the same fields and a new
//! setting only has to be added here once.

use std::collections::HashMap;

/// Merged snapshot of the server's state and player interfaces.
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
    /// The server has a live player but has not decoded a frame yet. The UI
    /// shows `Starting` instead of guessing between `Running` and `Idle`.
    pub is_decoding: bool,
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
    /// Monitor name → `1920 × 1080` as the layout reports it.
    pub monitor_resolutions: HashMap<String, String>,
    /// Monitor name → vertical refresh in hertz. Absent when the backend
    /// could not measure it, which the UI shows as no rate at all rather than
    /// as a guessed 60.
    pub monitor_refresh: HashMap<String, u32>,
    /// Config key of the primary display, empty when the server says nothing.
    pub primary_monitor: String,
    pub rotations: HashMap<String, String>,
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
    /// Decoder reported by the players (`hwdec-current`): API name, `no` for
    /// software, empty while nothing has decoded.
    pub hwdec_running: String,
    /// What the primary player's decoder reported: frame rate, codec,
    /// geometry, dropped frames, position.
    pub player_stats: ferramari_core::stats::PlaybackStats,
    pub web_allow_network: bool,
    pub autostart_enabled: bool,
    /// Whether the tray item is registered, which is what "run in background"
    /// means to the user: close the window and the daemon stays reachable.
    pub is_show_systray: bool,
    pub operation_status: String,
    pub operation_error: bool,
    /// Server-side sequence for `operation_status`; a repeated message with a
    /// new sequence is a new event.
    pub operation_seq: u64,
}
