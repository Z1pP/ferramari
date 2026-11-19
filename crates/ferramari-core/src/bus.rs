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
use crate::session::{IdleState, LockSource};
use crate::signing;
use crate::window::WindowState;
use crate::ytdlp;

#[derive(Debug, Clone)]
pub enum Command {
    /// Execute `command` and send its synchronous outcome to `reply`.
    ///
    /// Used by the control transports for the first application error after a
    /// UI action. Commands that complete on a worker later still report their
    /// progress through `operation_status`/`operation_error`.
    Request {
        command: Box<Command>,
        reply: mpsc::SyncSender<Result<(), String>>,
    },
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
    NextWallpaper,
    PreviousWallpaper,
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
    /// A lock facility reported its state. The source is part of the message
    /// because a session can have several, and one source's unlock must not
    /// cancel a lock another still holds; see `session::LockState`.
    LockChanged {
        source: LockSource,
        active: bool,
    },
    WindowState(WindowState),
    /// Desktop activity as reported by the session's idle facility.
    /// `Unknown` is a distinct answer, not a false: the service may be gone.
    Idle(IdleState),
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
    /// Show or hide the tray icon at runtime. Backs the "Run in background"
    /// switch in the control surface; the daemon itself keeps running either
    /// way, the tray is what makes it reachable without a window.
    SetShowSystray(bool),
    PowerStatus {
        on_battery: bool,
        percent: Option<u8>,
    },
    /// Charger plug / unplug delivered immediately (not through the 30 s poll).
    PowerEvent(crate::power::PowerEventType),
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

/// What the live players contribute to the global playback snapshot.
///
/// `playing` is true only for a live surface and IPC worker that the Eco plan
/// says should play. `decoding` separates "alive, waiting for the first frame"
/// from "nothing is running"; the two facts let a client show `Starting`
/// instead of guessing between `Running` and `Idle`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PlaybackFacts {
    pub playing: bool,
    pub decoding: bool,
}

impl PlaybackFacts {
    /// Combine one player's liveness, the Eco decision and the last probe.
    pub fn observe(player_ready: bool, plan_play: bool, decoded: bool) -> Self {
        let playing = player_ready && plan_play;
        Self {
            playing,
            decoding: playing && !decoded,
        }
    }

    /// Any live output counts as playing, and any output still waiting for
    /// its first frame keeps the global `decoding` flag set.
    pub fn merge(self, other: Self) -> Self {
        Self {
            playing: self.playing || other.playing,
            decoding: self.decoding || other.decoding,
        }
    }
}

/// Read-only view of the current state for the D-Bus interfaces.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub mode: Mode,
    pub volume: i64,
    pub blur_radius: i64,
    pub is_mute: bool,
    /// A live surface and IPC worker are running, and the Eco plan says the
    /// player should play. This is a fact about a live player, not a
    /// prediction of what the policy would like to do.
    pub is_playing: bool,
    /// `is_playing` is true, but the decoder has not produced a frame yet.
    /// This is the cold-start window that used to read as "not playing".
    pub is_decoding: bool,
    pub is_paused_by_user: bool,
    pub is_static_wallpaper: bool,
    pub is_pause_when_maximized: bool,
    pub is_mute_when_maximized: bool,
    pub theme: String,
    pub data_source: std::collections::HashMap<String, String>,
    /// Monitor name → Eco state label (`active`, `idle`, `occluded`,
    /// `suspended`). Useful to scripts and the Home cards.
    pub monitor_states: std::collections::HashMap<String, String>,
    /// Monitor name → `1920 × 1080` as the layout reports it. A plain string
    /// rather than a pair: the map travels over D-Bus, where a variant of two
    /// integers would force every client to learn a new type for one label.
    pub monitor_resolutions: std::collections::HashMap<String, String>,
    /// Monitor name → vertical refresh in hertz as whole digits (`144`).
    /// Absent for a monitor the backend could not measure — a Wayland session
    /// or a virtual output — which is why this is a map of names rather than
    /// a parallel vector: a missing display must not shift every later value
    /// into the wrong row.
    pub monitor_refresh: std::collections::HashMap<String, u32>,
    /// Config key of the primary display, empty when the backend says nothing.
    pub primary_monitor: String,
    /// What the primary player's decoder reported at the last supervision
    /// tick: frame rate, codec, geometry, dropped frames. Empty while nothing
    /// is decoding.
    pub player_stats: crate::stats::PlaybackStats,
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
    /// Most recent long-running operation result for UI clients.
    pub operation_status: String,
    pub operation_error: bool,
    /// Incremented whenever a status is published, so a client can tell a
    /// repeated message from a new occurrence.
    pub operation_seq: u64,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            mode: Mode::Null,
            volume: 50,
            blur_radius: 5,
            is_mute: false,
            is_playing: false,
            is_decoding: false,
            is_paused_by_user: false,
            is_static_wallpaper: true,
            is_pause_when_maximized: true,
            is_mute_when_maximized: false,
            theme: "system".to_string(),
            data_source: std::collections::HashMap::new(),
            monitor_states: std::collections::HashMap::new(),
            monitor_resolutions: std::collections::HashMap::new(),
            monitor_refresh: std::collections::HashMap::new(),
            primary_monitor: String::new(),
            player_stats: crate::stats::PlaybackStats::default(),
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
            operation_seq: 0,
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

#[cfg(test)]
mod tests {
    use super::PlaybackFacts;

    #[test]
    fn decoding_needs_a_live_player_that_has_not_decoded_yet() {
        assert_eq!(
            PlaybackFacts::observe(false, true, false),
            PlaybackFacts::default(),
            "a dead player is not playing or decoding"
        );
        assert_eq!(
            PlaybackFacts::observe(true, false, false),
            PlaybackFacts::default(),
            "the Eco plan can stop playback without touching the process"
        );
        assert_eq!(
            PlaybackFacts::observe(true, true, false),
            PlaybackFacts {
                playing: true,
                decoding: true
            }
        );
        assert_eq!(
            PlaybackFacts::observe(true, true, true),
            PlaybackFacts {
                playing: true,
                decoding: false
            }
        );
    }

    #[test]
    fn merging_keeps_any_playing_and_any_starting() {
        let running = PlaybackFacts::observe(true, true, true);
        let starting = PlaybackFacts::observe(true, true, false);
        assert_eq!(
            running.merge(starting),
            PlaybackFacts {
                playing: true,
                decoding: true
            },
            "one output still starting is visible in the global fact"
        );
        assert_eq!(PlaybackFacts::default().merge(running), running);
    }
}
