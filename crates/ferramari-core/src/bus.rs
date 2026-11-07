//! The command bus: every part of the program that wants to change something
//! (UI callbacks, D-Bus methods, X11/session watchers, tray) sends a
//! [`Command`] into one channel. The owning thread drains it and mutates the
//! application state, so state changes are serialised in one place.
//!
//! This module is deliberately toolkit-free: the Iced control surface and platform event sources both drain the same channel.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender as StdSender};

use crate::config::Mode;
use crate::package;
use crate::signing;
use crate::window::WindowState;
use crate::ytdlp;

#[derive(Debug, Clone)]
pub enum Command {
    ShowGui,
    Null,
    Video {
        path: Option<String>,
        monitor: Option<String>,
    },
    Stream {
        url: String,
    },
    StreamReady {
        url: String,
        result: Result<ytdlp::StreamInfo, String>,
    },
    Webpage {
        url: String,
    },
    WebpageReady {
        url: String,
        result: Result<(), String>,
    },
    PausePlayback,
    StartPlayback,
    Reload,
    ReloadConfig,
    QuitPlayer,
    FeelingLucky,
    Quit,
    ToggleMute,
    TogglePause,
    SetVolume(i64),
    SetMute(bool),
    SetPausedByUser(bool),
    SetBlurRadius(i64),
    SetStaticWallpaper(bool),
    SetPauseWhenMaximized(bool),
    SetMuteWhenMaximized(bool),
    ScreenLockActive(bool),
    SessionLock(bool),
    WindowState(WindowState),
    Idle(bool),
    MonitorsChanged,
    MonitorsChangedNow,
    SetAutostart(bool),
    SetLanguage(String),
    SetTheme(String),
    SetRotationMode(String),
    SetRotationInterval(i64),
    SetMonitorRotation {
        monitor: String,
        collection: Option<String>,
    },
    SetPlaybackProfile(String),
    SetPauseOnBattery(bool),
    SetIdleTimeout(i64),
    SetHardwareDecoding(String),
    SetWebNetwork(bool),
    PowerStatus {
        on_battery: bool,
        percent: Option<u8>,
    },
    Suspend(bool),
    LibraryChanged,
    ImportPackage(PathBuf),
    PackageImported(Result<Box<package::ImportedPackage>, String>),
    ExportPackage {
        video: PathBuf,
        parent: PathBuf,
    },
    PackageExported(Result<PathBuf, String>),
    SignPackage(PathBuf),
    PackageSigned(Result<PathBuf, String>),
    VerifyPackage(PathBuf),
    PackageVerified(Box<PackageReport>),
    TrustKey(PathBuf),
    WebError {
        monitor: String,
        message: String,
    },
    StaticWallpaperReady {
        source: String,
        blur: i64,
        generation: u64,
        file: Option<PathBuf>,
    },
}

/// Integrity and authorship of one package, for the library dialog.
#[derive(Debug, Clone)]
pub struct PackageReport {
    pub dir: PathBuf,
    pub checksummed: bool,
    pub signature: signing::SignatureStatus,
}

/// Read-only view of the current state for the D-Bus interfaces.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub mode: Mode,
    pub volume: i64,
    pub blur_radius: i64,
    pub is_mute: bool,
    pub is_playing: bool,
    pub is_paused_by_user: bool,
    pub is_static_wallpaper: bool,
    pub is_pause_when_maximized: bool,
    pub is_mute_when_maximized: bool,
    pub theme: String,
    pub data_source: std::collections::HashMap<String, String>,
    /// Monitor name → Eco state label (`active`, `idle`, `occluded`,
    /// `suspended`). Useful to scripts and the Home cards.
    pub monitor_states: std::collections::HashMap<String, String>,
    /// Settings the control surface edits, mirrored here so a bus client
    /// (the Iced UI) can read and change them without file access.
    pub language: String,
    pub rotation_mode: String,
    pub rotation_interval_sec: i64,
    pub playback_profile: String,
    pub pause_on_battery: bool,
    pub idle_timeout_sec: i64,
    pub hwdec: String,
    /// The decoder the players report as actually running (`hwdec-current`):
    /// an API name, `no` for software, or empty while nothing has decoded.
    /// The setting above is what the user asked for; this is the machine's
    /// answer, and the UI compares them.
    pub hwdec_running: String,
    pub web_allow_network: bool,
    pub fade_duration_sec: f64,
    pub fade_interval: f64,
    pub is_show_systray: bool,
    pub autostart_enabled: bool,
    /// Monitor name → rotation collection id.
    pub rotations: std::collections::HashMap<String, String>,
    /// Most recent long-running package operation result for UI clients.
    pub operation_status: String,
    pub operation_error: bool,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            mode: Mode::Null,
            volume: 50,
            blur_radius: 5,
            is_mute: false,
            is_playing: false,
            is_paused_by_user: false,
            is_static_wallpaper: true,
            is_pause_when_maximized: true,
            is_mute_when_maximized: false,
            theme: "system".to_string(),
            data_source: std::collections::HashMap::new(),
            monitor_states: std::collections::HashMap::new(),
            language: String::new(),
            rotation_mode: "off".to_string(),
            rotation_interval_sec: 300,
            playback_profile: "balanced".to_string(),
            pause_on_battery: false,
            idle_timeout_sec: 300,
            hwdec: "auto".to_string(),
            hwdec_running: String::new(),
            web_allow_network: true,
            fade_duration_sec: 1.0,
            fade_interval: 0.05,
            is_show_systray: true,
            autostart_enabled: false,
            rotations: std::collections::HashMap::new(),
            operation_status: String::new(),
            operation_error: false,
        }
    }
}

/// Cloneable command handle shared with UI, D-Bus, tray and watcher threads.
/// The standard channel wakes the owning event loop when a command arrives.
#[derive(Clone)]
pub struct CommandSender(StdSender<Command>);

impl CommandSender {
    pub fn new(sender: StdSender<Command>) -> Self {
        Self(sender)
    }

    pub fn send(&self, command: Command) -> Result<(), mpsc::SendError<Command>> {
        self.0.send(command)
    }
}

/// Convenience alias for the drain side of the channel.
pub type CommandReceiver = Receiver<Command>;
