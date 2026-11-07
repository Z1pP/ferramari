//! Application state and the command loop.
//!
//! Every part of the program that wants to change something (D-Bus methods,
//! X11/session watchers, tray) sends a [`Command`] into one channel. A single
//! thread drains it and mutates [`App`], so state changes are serialised in
//! one place and nothing here needs `Send`.
//!
//! The loop is a plain `recv_timeout` over the command channel plus deadline
//! bookkeeping for what used to be GLib timeouts. Periodic work therefore runs
//! on the same thread that owns the players, exactly like the previous GTK
//! main loop, without a second event loop in the process.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use rand::seq::SliceRandom;

use crate::autostart;
use crate::config::{Config, Mode};
use crate::eco;
use crate::error::{ErrorKind, UserError};
use crate::handlers;
use crate::library::Library;
use crate::monitors::{self, MonitorInfo};
use crate::package;
use crate::paths;
use crate::power;
use crate::signing;
use crate::supervisor::{RestartPolicy, Retry, MAX_RETRIES};
use crate::tray;
use crate::util;
use crate::wallpaper::GnomeBackground;
use crate::x11::{self, WindowState};
use crate::ytdlp;

use ferramari_media::engine::{ContentKind, Engine, WindowKey};

/// How long a hidden local video must stay paused before its decoder is
/// released. Five minutes keeps short game alt-tabs and lock-screen blips from
/// paying a reload; the mpv process itself is kept alive.
const DECODER_PARK_AFTER: Duration = Duration::from_secs(5 * 60);

/// Cadence of the periodic jobs that were GLib timeouts.
const PLAYER_WATCH_EVERY: Duration = Duration::from_secs(1);
const ROTATION_WATCH_EVERY: Duration = Duration::from_secs(5);
const POWER_WATCH_EVERY: Duration = Duration::from_secs(30);
/// Coalescing window for a hotplug burst, and the debounce for a burst of
/// slider changes waiting to be written to disk.
const MONITOR_DEBOUNCE: Duration = Duration::from_millis(250);
const CONFIG_FLUSH_AFTER: Duration = Duration::from_millis(500);

// The command bus (Command, PackageReport, Snapshot, CommandSender)
// lives in ferramari-core. Re-export it under `app` so the pre-split call
// sites (`crate::app::Command`, …) keep resolving.
pub use crate::bus::{Command, CommandSender, PackageReport, Snapshot};

pub struct App {
    pub config: Config,
    pub monitors: Vec<MonitorInfo>,
    pub windows: Vec<Engine>,
    pub snapshot: Arc<Mutex<Snapshot>>,
    tx: CommandSender,
    weak_self: Weak<RefCell<App>>,
    background: bool,
    paused_by_user: bool,
    /// Aggregate lock state; a lock from either source keeps it true.
    screen_locked: bool,
    screensaver_locked: bool,
    logind_locked: bool,
    suspended: bool,
    /// Set from desktop idle time; only the Eco profile stops for it.
    idle: bool,
    /// Latest per-window occlusion sample from the X11 watcher.
    window_state: WindowState,
    /// Last Eco decision applied to each monitor, so a state change only touches
    /// the windows whose state or FPS cap actually changed.
    last_plans: HashMap<String, eco::Plan>,
    stream: Option<ytdlp::StreamInfo>,
    stream_resolving: bool,
    background_wallpaper: GnomeBackground,
    library: Rc<RefCell<Library>>,
    last_rotation: HashMap<String, Instant>,
    on_battery: bool,
    battery_percent: Option<u8>,
    /// Shared with the idle watcher; a settings change takes effect on the
    /// next 15 s poll without restarting the thread.
    idle_threshold: Arc<AtomicU64>,
    static_job: Option<(String, i64)>,
    static_generation: u64,
    /// Set when a wallpaper window is newly mapped: only then do the desktop
    /// icons need to be redrawn on top of it.
    pending_icon_refresh: bool,
    /// Pending debounced config save; volume and blur sliders change fast.
    config_dirty_at: Option<Instant>,
    policies: HashMap<WindowKey, RestartPolicy>,
    /// Kept alive so the EWMH watcher stops with the app.
    window_watcher: Option<x11::Watcher>,
    /// Kept alive so the RandR hotplug watcher stops with the app.
    monitor_watcher: Option<monitors::Watcher>,
    /// When a debounced hotplug should be applied, from `Command::MonitorsChanged`.
    monitor_due: Option<Instant>,
    quit_requested: bool,
    gui_shown: bool,
}

impl App {
    pub fn new(
        args: &crate::cli::Args,
        tx: CommandSender,
        snapshot: Arc<Mutex<Snapshot>>,
    ) -> Rc<RefCell<Self>> {
        paths::ensure_dirs();
        thread::Builder::new()
            .name("ferramari-cache-clean".into())
            .spawn(|| {
                let age = Duration::from_secs(3600);
                util::cleanup_partial_files(&paths::thumbnail_cache_dir(), age);
                util::cleanup_partial_files(&paths::static_cache_dir(), age);
            })
            .ok();
        let monitors = monitors::list();
        let names: Vec<String> = monitors
            .iter()
            .map(|monitor| monitor.name.clone())
            .collect();
        let config = Config::load(&names);
        let background_wallpaper = GnomeBackground::capture();
        // Read the power supply once: two separate `status()` calls can
        // observe different states while a charger is being plugged in.
        let power_status = power::status();
        let idle_threshold = Arc::new(AtomicU64::new(
            config.idle_timeout_sec.clamp(30, 86_400) as u64,
        ));

        let app = Self {
            config,
            monitors,
            windows: Vec::new(),
            snapshot,
            tx: tx.clone(),
            weak_self: Weak::new(),
            background: args.background,
            paused_by_user: false,
            screen_locked: false,
            screensaver_locked: false,
            logind_locked: false,
            suspended: false,
            idle: false,
            window_state: WindowState::default(),
            last_plans: HashMap::new(),
            stream: None,
            stream_resolving: false,
            background_wallpaper,
            library: Rc::new(RefCell::new(Library::load())),
            last_rotation: HashMap::new(),
            on_battery: power_status.on_battery,
            battery_percent: power_status.percent,
            idle_threshold,
            static_job: None,
            static_generation: 0,
            pending_icon_refresh: false,
            config_dirty_at: None,
            policies: HashMap::new(),
            window_watcher: None,
            monitor_watcher: None,
            monitor_due: None,
            quit_requested: false,
            gui_shown: false,
        };
        app.sync_snapshot();

        if app.config.is_show_systray {
            tray::spawn(tx.clone());
        }

        let app = Rc::new(RefCell::new(app));
        app.borrow_mut().weak_self = Rc::downgrade(&app);
        app.borrow_mut().start_external_watchers();
        app
    }

    /// Drain commands and run the periodic jobs until `Command::Quit` arrives
    /// or the channel closes. This is the whole event loop: one thread owns
    /// the players, so nothing here needs locking or message passing.
    pub fn run(app: &Rc<RefCell<App>>, rx: Receiver<Command>) {
        let mut next_player = Instant::now();
        let mut next_rotation = Instant::now() + ROTATION_WATCH_EVERY;
        let mut next_power = Instant::now() + POWER_WATCH_EVERY;
        let mut previous_power = power::status();

        while !app.borrow().quit_requested {
            // Sleep only until the earliest deadline, so an idle process still
            // reacts to a command immediately.
            let now = Instant::now();
            let mut wait = next_player
                .min(next_rotation)
                .min(next_power)
                .saturating_duration_since(now);
            if let Some(due) = app.borrow().monitor_due {
                wait = wait.min(due.saturating_duration_since(now));
            }
            match rx.recv_timeout(wait) {
                Ok(command) => app.borrow_mut().handle(command),
                // Every sender lives in this process; a dropped channel means
                // the watchers are gone and nothing can wake us again.
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            // Everything queued behind the command we just took.
            while let Ok(command) = rx.try_recv() {
                app.borrow_mut().handle(command);
                if app.borrow().quit_requested {
                    break;
                }
            }

            let now = Instant::now();
            if now >= next_player {
                next_player = now + PLAYER_WATCH_EVERY;
                let mut state = app.borrow_mut();
                state.watch_players();
                state.flush_config_if_due();
            }
            if now >= next_rotation {
                next_rotation = now + ROTATION_WATCH_EVERY;
                app.borrow_mut().rotate_wallpapers();
            }
            if now >= next_power {
                next_power = now + POWER_WATCH_EVERY;
                let status = power::status();
                if status != previous_power {
                    previous_power = status;
                    app.borrow_mut().handle(Command::PowerStatus {
                        on_battery: status.on_battery,
                        percent: status.percent,
                    });
                }
            }
            // The hotplug debounce is a deadline, not a thread: a burst of
            // RandR notifications collapses into one rebuild.
            let due = app.borrow().monitor_due;
            if due.is_some_and(|due| now >= due) {
                app.borrow_mut().monitor_due = None;
                app.borrow_mut().handle(Command::MonitorsChangedNow);
            }
        }

        app.borrow_mut().shutdown();
    }

    fn start_external_watchers(&mut self) {
        let tx = self.tx.clone();
        let idle_threshold = Arc::clone(&self.idle_threshold);

        handlers::start_screen_lock_watcher(tx.clone());
        handlers::start_end_session_watcher(tx.clone());
        handlers::start_sleep_watcher(tx.clone());
        handlers::start_logind_lock_watcher(tx.clone());
        handlers::start_idle_watcher(tx.clone(), idle_threshold);
        // The app always runs as an X11/XWayland client, so the EWMH watcher is
        // useful on a Wayland session too: X11 fullscreen windows are listed
        // in _NET_CLIENT_LIST even there. `x11::start` returns `None` when no
        // X server answers at all, which is the only real failure mode.
        self.window_watcher = x11::start(tx.clone());
        // RandR replaces the GDK monitor signals: the watcher thread only ever
        // touches the command channel.
        self.monitor_watcher = monitors::start(tx);
    }

    pub fn activate(&mut self) {
        self.rebuild_windows();
        if !self.background {
            self.show_gui();
        }
    }

    pub fn shutdown(&mut self) {
        self.flush_config();
        self.close_windows();
        self.background_wallpaper.restore();
        self.window_watcher = None;
        self.monitor_watcher = None;
    }

    fn handle(&mut self, command: Command) {
        match command {
            Command::ShowGui => self.show_gui(),
            Command::Null => {
                self.config.mode = Mode::Null;
                self.save_config();
                self.rebuild_windows();
            }
            Command::Video { path, monitor } => self.apply_video(path, monitor),
            Command::Stream { url } => self.resolve_stream(url),
            Command::StreamReady { url, result } => {
                self.stream_resolving = false;
                match result {
                    Ok(info) => {
                        self.adopt_stream(url, info);
                        self.rebuild_windows();
                    }
                    Err(err) => {
                        self.stream = None;
                        self.notify_user_error(UserError::new(ErrorKind::Network, err));
                        self.sync_snapshot();
                    }
                }
            }
            Command::Webpage { url } => self.start_webpage(url),
            Command::WebpageReady { url, result } => match result {
                Ok(()) => {
                    self.config.mode = Mode::Webpage;
                    self.config.data_source.insert("Default".to_string(), url);
                    self.save_config();
                    self.rebuild_windows();
                }
                Err(err) => self.notify_user_error(UserError::new(ErrorKind::Network, err)),
            },
            Command::PausePlayback => {
                self.pause_all();
                self.sync_snapshot();
            }
            Command::StartPlayback => {
                self.apply_playback_states();
                self.sync_snapshot();
            }
            Command::Reload => {
                if self.config.mode == Mode::Stream {
                    self.stream = None;
                }
                self.restart_windows();
            }
            Command::ReloadConfig => self.reload_config(),
            Command::QuitPlayer => self.quit_player(),
            Command::FeelingLucky => self.feeling_lucky(),
            Command::Quit => self.quit(),
            Command::ToggleMute => self.set_mute(!self.config.is_mute),
            Command::TogglePause => self.set_paused_by_user(!self.paused_by_user),
            Command::SetVolume(volume) => self.set_volume(volume),
            Command::SetMute(muted) => self.set_mute(muted),
            Command::SetPausedByUser(paused) => self.set_paused_by_user(paused),
            Command::SetBlurRadius(radius) => self.set_blur_radius(radius),
            Command::SetStaticWallpaper(enabled) => self.set_static_wallpaper(enabled),
            Command::SetPauseWhenMaximized(enabled) => {
                self.config.is_pause_when_maximized = enabled;
                self.save_config();
                self.apply_playback_states();
                self.apply_mute_states();
            }
            Command::SetMuteWhenMaximized(enabled) => {
                self.config.is_mute_when_maximized = enabled;
                self.save_config();
                self.apply_mute_states();
            }
            Command::ScreenLockActive(active) => {
                log::debug!("[ScreenLock] screensaver active={active}");
                self.screensaver_locked = active;
                self.screen_locked = self.screensaver_locked || self.logind_locked;
                self.apply_playback_states();
            }
            Command::SessionLock(active) => {
                log::debug!("[ScreenLock] logind lock active={active}");
                self.logind_locked = active;
                self.screen_locked = self.screensaver_locked || self.logind_locked;
                self.apply_playback_states();
            }
            Command::WindowState(state) => {
                if state == self.window_state {
                    return;
                }
                self.window_state = state;
                log::debug!(
                    "[WindowState] {} visible window(s)",
                    self.window_state.windows.len()
                );
                self.apply_playback_states();
                self.apply_mute_states();
            }
            Command::Idle(idle) => {
                if self.idle != idle {
                    log::debug!("[Eco] idle={idle}");
                    self.idle = idle;
                    self.apply_playback_states();
                }
            }
            Command::SetAutostart(enabled) => self.set_autostart(enabled),
            Command::SetLanguage(language) => {
                if self.config.language != language {
                    self.config.language = language;
                    self.save_config();
                }
            }
            Command::SetTheme(mode) => {
                let mode = if matches!(mode.as_str(), "light" | "dark") {
                    mode
                } else {
                    "system".to_string()
                };
                if self.config.theme != mode {
                    self.config.theme = mode;
                    self.save_config();
                }
            }
            Command::SetRotationMode(mode) => {
                if self.config.rotation_mode != mode {
                    self.config.rotation_mode = mode;
                    self.last_rotation.clear();
                    self.save_config();
                }
            }
            Command::SetRotationInterval(seconds) => {
                let seconds = seconds.clamp(10, 86_400);
                if self.config.rotation_interval_sec != seconds {
                    self.config.rotation_interval_sec = seconds;
                    self.last_rotation.clear();
                    self.save_config();
                }
            }
            Command::SetPlaybackProfile(profile) => {
                // Accept `quality`/`eco` from newer clients and keep the
                // documented legacy identifiers on disk.
                let canonical = eco::Profile::from_config(&profile)
                    .config_value()
                    .to_string();
                if self.config.playback_profile != canonical {
                    self.config.playback_profile = canonical;
                    self.save_config();
                    self.restart_windows();
                }
            }
            Command::SetPauseOnBattery(enabled) => {
                self.config.pause_on_battery = enabled;
                self.save_config();
                self.apply_playback_states();
            }
            Command::SetIdleTimeout(seconds) => {
                let seconds = seconds.clamp(30, 86_400);
                if self.config.idle_timeout_sec != seconds {
                    self.config.idle_timeout_sec = seconds;
                    self.idle_threshold.store(seconds as u64, Ordering::Relaxed);
                    self.save_config();
                }
            }
            Command::SetHardwareDecoding(mode) => {
                let mode = match mode.as_str() {
                    "vaapi" => "vaapi",
                    "none" => "none",
                    _ => "auto",
                }
                .to_string();
                if self.config.hwdec != mode {
                    self.config.hwdec = mode;
                    self.save_config();
                    // The decoder mode is a process-start flag in mpv.
                    self.restart_windows();
                }
            }
            Command::SetWebNetwork(enabled) => {
                if self.config.web_allow_network != enabled {
                    self.config.web_allow_network = enabled;
                    self.save_config();
                    if self.config.mode == Mode::Webpage {
                        self.restart_windows();
                    }
                }
            }
            Command::Suspend(sleeping) => {
                log::info!("[Power] Suspend={sleeping}");
                self.suspended = sleeping;
                self.apply_playback_states();
            }
            Command::PowerStatus {
                on_battery,
                percent,
            } => {
                let changed = self.on_battery != on_battery || self.battery_percent != percent;
                self.on_battery = on_battery;
                self.battery_percent = percent;
                if changed && self.config.pause_on_battery {
                    self.apply_playback_states();
                }
            }
            Command::LibraryChanged => {}
            Command::ImportPackage(source) => {
                let tx = self.tx.clone();
                let destination = paths::packages_dir();
                thread::Builder::new()
                    .name("ferramari-package-import".to_string())
                    .spawn(move || {
                        let result = package::import(&source, &destination).map(Box::new);
                        let _ = tx.send(Command::PackageImported(result));
                    })
                    .ok();
            }
            Command::PackageImported(result) => match result {
                Ok(imported) => {
                    log::info!(
                        "[Package] Imported {} ({}, {} bytes, preview={}, checksums={}, {})",
                        imported.name,
                        imported.kind.as_str(),
                        imported.size_bytes,
                        imported.preview.is_some(),
                        imported.checksummed,
                        imported.signature.label()
                    );
                    match imported.kind {
                        package::PackageType::Video => {
                            let source = imported.entry.to_string_lossy().into_owned();
                            if assign_video(&mut self.config, &self.monitors, source.clone(), None)
                            {
                                self.save_config();
                                self.library.borrow_mut().record_use(&source);
                                let _ = self.library.borrow().save();
                                self.rebuild_windows();
                            }
                        }
                        package::PackageType::Web => {
                            self.config.mode = Mode::Webpage;
                            self.config
                                .data_source
                                .insert("Default".to_string(), util::path_to_uri(&imported.entry));
                            self.save_config();
                            self.rebuild_windows();
                        }
                    }
                }
                Err(err) => self.notify_user_error(UserError::new(
                    ErrorKind::FileSystem,
                    format!("Could not import the package: {err}"),
                )),
            },
            Command::ExportPackage { video, parent } => {
                let tx = self.tx.clone();
                thread::Builder::new()
                    .name("ferramari-package-export".to_string())
                    .spawn(move || {
                        let result = package::export_video(&video, &parent);
                        let _ = tx.send(Command::PackageExported(result));
                    })
                    .ok();
            }
            Command::PackageExported(result) => match result {
                Ok(path) => log::info!("[Package] Exported {}", path.display()),
                Err(err) => self.notify_user_error(UserError::new(
                    ErrorKind::FileSystem,
                    format!("Could not export the package: {err}"),
                )),
            },
            Command::SignPackage(dir) => {
                let tx = self.tx.clone();
                thread::Builder::new()
                    .name("ferramari-package-sign".to_string())
                    .spawn(move || {
                        let result = (|| {
                            let key = signing::load_or_create_key(&signing::default_author_name())?;
                            package::write_checksums(&dir)?;
                            signing::sign_package(&dir, &key)?;
                            package::load(&dir).map(|package| package.root)
                        })();
                        let _ = tx.send(Command::PackageSigned(result));
                    })
                    .ok();
            }
            Command::PackageSigned(result) => match result {
                Ok(dir) => log::info!("[Package] Signed {}", dir.display()),
                Err(err) => self.notify_user_error(UserError::new(
                    ErrorKind::FileSystem,
                    format!("Could not sign the package: {err}"),
                )),
            },
            Command::VerifyPackage(dir) => {
                let tx = self.tx.clone();
                thread::Builder::new()
                    .name("ferramari-package-verify".to_string())
                    .spawn(move || {
                        let result = (|| -> Result<PackageReport, String> {
                            let package = package::load(&dir)?;
                            Ok(PackageReport {
                                checksummed: package.checksummed,
                                signature: package.signature,
                                dir: package.root,
                            })
                        })();
                        match result {
                            Ok(report) => {
                                let _ = tx.send(Command::PackageVerified(Box::new(report)));
                            }
                            Err(err) => {
                                let _ =
                                    tx.send(Command::PackageVerified(Box::new(PackageReport {
                                        dir,
                                        checksummed: false,
                                        signature: signing::SignatureStatus::Invalid {
                                            key_id: None,
                                            reason: err,
                                        },
                                    })));
                            }
                        }
                    })
                    .ok();
            }
            Command::PackageVerified(report) => {
                log::info!(
                    "[Package] {} checksums={} {}",
                    report.dir.display(),
                    report.checksummed,
                    report.signature.label()
                );
            }
            Command::TrustKey(path) => match signing::trust_source(&path) {
                Ok(Some(key_id)) => log::info!("[Package] Trusted key {key_id}"),
                Ok(None) => self.notify_user_error(UserError::new(
                    ErrorKind::FileSystem,
                    format!("{} carries no key to trust", path.display()),
                )),
                Err(err) => self.notify_user_error(UserError::new(
                    ErrorKind::FileSystem,
                    format!("Could not trust the key: {err}"),
                )),
            },
            Command::SetMonitorRotation {
                monitor,
                collection,
            } => {
                if !self.monitors.iter().any(|info| info.name == monitor) {
                    return;
                }
                match collection {
                    Some(id) if self.library.borrow().collection(&id).is_some() => {
                        self.config.rotations.insert(monitor.clone(), id);
                    }
                    _ => {
                        self.config.rotations.remove(&monitor);
                    }
                }
                self.last_rotation.remove(&monitor);
                self.save_config();
            }
            Command::WebError { monitor, message } => {
                log::warn!("[Web] {monitor}: {message}");
                self.notify_user_error(UserError::with_detail(
                    ErrorKind::Network,
                    format!("Could not load the webpage on {monitor}"),
                    message,
                ));
            }
            Command::StaticWallpaperReady {
                source,
                blur,
                generation,
                file,
            } => {
                if generation != self.static_generation {
                    if let Some(file) = file {
                        let _ = fs::remove_file(file);
                    }
                    return;
                }
                let still_current = self.config.mode == Mode::Video
                    && self.config.is_static_wallpaper
                    && self.config.default_source() == source
                    && self.config.blur_radius.max(0) == blur;
                if !still_current {
                    if let Some(file) = file {
                        let _ = fs::remove_file(file);
                    }
                    return;
                }
                match file {
                    Some(file) => {
                        if self
                            .background_wallpaper
                            .apply_static_frame(&file)
                            .is_none()
                        {
                            let _ = fs::remove_file(&file);
                        }
                    }
                    None => {
                        self.static_job = None;
                        log::warn!("[StaticWallpaper] Frame generation failed");
                    }
                }
            }
            Command::MonitorsChanged => {
                // A single hotplug often arrives as added + removed + size
                // events. Coalesce them so the players are rebuilt once.
                let due = self
                    .monitor_due
                    .unwrap_or_else(|| Instant::now() + MONITOR_DEBOUNCE);
                self.monitor_due = Some(due);
            }
            Command::MonitorsChangedNow => {
                self.monitor_due = None;
                log::info!("[Monitors] Layout changed");
                // A hotplug must not throw away a debounced volume/blur edit:
                // `Config::load` re-reads the file, which does not contain it
                // yet. Flush first, then merge the new monitor names.
                self.flush_config();
                self.monitors = monitors::list();
                let names: Vec<String> = self
                    .monitors
                    .iter()
                    .map(|monitor| monitor.name.clone())
                    .collect();
                self.config = Config::load(&names);
                self.idle_threshold.store(
                    self.config.idle_timeout_sec.clamp(30, 86_400) as u64,
                    Ordering::Relaxed,
                );
                self.rebuild_windows();
            }
        }
        self.sync_snapshot();
    }

    fn apply_video(&mut self, path: Option<String>, monitor: Option<String>) {
        if let Some(path) = path {
            if !Path::new(&path).is_file() {
                self.notify_user_error(UserError::new(
                    ErrorKind::FileSystem,
                    format!("File not found: {path}"),
                ));
                return;
            }
            let used_path = path.clone();
            if !assign_video(&mut self.config, &self.monitors, path, monitor.as_deref()) {
                return;
            }
            self.save_config();
            self.library.borrow_mut().record_use(&used_path);
            if let Err(err) = self.library.borrow().save() {
                log::warn!("[Library] {err}");
            }
        }
        self.rebuild_windows();
    }

    /// Highest monitor height, which decides the stream format to ask for.
    fn screen_height(&self) -> u32 {
        self.monitors
            .iter()
            .map(|monitor| monitor.height.max(0) as u32)
            .max()
            .unwrap_or(1080)
    }

    /// Make a resolved stream current. The config is only written when the
    /// source actually changed: `Reload` re-adopts the same URL, and rewriting
    /// the file (plus its backup) on every refresh is pointless.
    fn adopt_stream(&mut self, url: String, info: ytdlp::StreamInfo) {
        log::info!("[Stream] {}x{} from {url}", info.width, info.height);
        let changed =
            self.config.mode != Mode::Stream || self.config.default_source() != url.as_str();
        self.stream = Some(info);
        self.config.mode = Mode::Stream;
        self.config.data_source.insert("Default".to_string(), url);
        if changed {
            self.save_config();
        }
    }

    /// Resolve a stream URL in the background. The mode only changes once a
    /// playable URL is known, so a bad link leaves the current wallpaper alone.
    fn resolve_stream(&mut self, url: String) {
        if self.stream_resolving {
            return;
        }
        let screen_height = self.screen_height();
        // A stream resolved in the last few minutes is still playable. Taking
        // it synchronously skips `yt-dlp -J` (1.5–2.7 s) and, more visibly,
        // the blank desktop a `Reload` would otherwise show while the worker
        // runs.
        if let Some(info) = ytdlp::cached(&url, screen_height) {
            log::info!("[Stream] Reusing the cached resolution for {url}");
            self.adopt_stream(url, info);
            self.rebuild_windows();
            return;
        }
        self.stream_resolving = true;
        let tx = self.tx.clone();
        log::info!("[Stream] Resolving: {url}");
        thread::spawn(move || {
            let result = ytdlp::resolve(&url, screen_height);
            let _ = tx.send(Command::StreamReady { url, result });
        });
    }

    fn start_webpage(&mut self, url: String) {
        if url.is_empty() {
            return;
        }
        if url.starts_with("http://") || url.starts_with("https://") {
            let target = url.clone();
            let tx = self.tx.clone();
            thread::spawn(move || {
                let result = ytdlp::check_url(&target);
                let _ = tx.send(Command::WebpageReady {
                    url: target,
                    result,
                });
            });
            return;
        }
        // `file://` is a local document, not a network resource. Running it
        // through `check_url` made D-Bus callers see a network error even
        // though the requested HTML existed on disk.
        let Some(path) = local_webpage_path(&url) else {
            self.notify_user_error(UserError::new(
                ErrorKind::FileSystem,
                format!("File not found: {url}"),
            ));
            return;
        };
        if !path.is_file() {
            self.notify_user_error(UserError::new(
                ErrorKind::FileSystem,
                format!("File not found: {}", path.display()),
            ));
            return;
        }
        self.config.mode = Mode::Webpage;
        self.config
            .data_source
            .insert("Default".to_string(), util::path_to_uri(&path));
        self.save_config();
        self.rebuild_windows();
    }

    fn feeling_lucky(&mut self) {
        let videos = util::video_paths();
        let Some(video) = util::random_other_video(&videos, self.config.default_source())
            .or_else(|| videos.first().cloned())
        else {
            self.notify_user_error(UserError::new(
                ErrorKind::FileSystem,
                "No videos found in the Ferramari folder",
            ));
            return;
        };
        let video = video.to_string_lossy().to_string();
        log::info!("[Lucky] {video}");
        self.config.mode = Mode::Video;
        for monitor in &self.monitors {
            self.config
                .data_source
                .insert(monitor.name.clone(), video.clone());
        }
        self.library.borrow_mut().record_use(&video);
        if let Err(err) = self.library.borrow().save() {
            log::warn!("[Library] {err}");
        }
        self.config.data_source.insert("Default".to_string(), video);
        self.save_config();
        self.rebuild_windows();
    }

    fn rebuild_windows(&mut self) {
        // Streaming URLs are signed and expire: a persisted page URL has to be
        // resolved again before any window can be recreated.
        if self.config.mode == Mode::Stream && self.stream.is_none() {
            let url = self.config.default_source().to_string();
            if !url.is_empty() {
                // Adopt a recent resolution in place instead of entering
                // `resolve_stream`, which rebuilds the windows itself and
                // would make this call recurse.
                if let Some(info) = ytdlp::cached(&url, self.screen_height()) {
                    log::info!("[Stream] Reusing the cached resolution for {url}");
                    self.adopt_stream(url, info);
                } else {
                    self.resolve_stream(url);
                }
            }
        }
        self.sync_windows();
        self.finish_rebuild();
    }

    /// Drop every player and rebuild from scratch. Used by `Reload`, where
    /// restarting the current source is the point (e.g. a file changed on disk).
    fn restart_windows(&mut self) {
        self.close_windows();
        self.rebuild_windows();
    }

    /// Make the players match the config, keeping windows whose key did not
    /// change. A per-monitor video change therefore leaves the other monitors
    /// playing instead of restarting every decoder.
    fn sync_windows(&mut self) {
        let now = Instant::now();
        let desired = self.desired_windows();
        self.policies.retain(|key, _| desired.contains(key));
        // While a stream is being re-resolved the old windows have no
        // replacement yet; keep them playing rather than showing a black
        // desktop, then swap once `StreamReady` arrives.
        let pending_stream = self.config.mode == Mode::Stream && self.stream.is_none();
        self.windows.retain(|window| {
            desired.contains(window.key())
                // A video player can be reused for a new file on the same
                // monitor instead of being torn down and respawned.
                || (window.key().kind == ContentKind::Video
                    && desired.iter().any(|key| {
                        key.kind == ContentKind::Video
                            && key.monitor == window.key().monitor
                            && Path::new(&key.source).is_file()
                    }))
                || (pending_stream && window.key().kind == ContentKind::Stream)
        });

        let live_monitors: Vec<String> = self
            .windows
            .iter()
            .map(|window| window.key().monitor.clone())
            .collect();
        self.last_plans
            .retain(|monitor, _| live_monitors.contains(monitor));

        let volume = self.config.volume;
        let muted = self.config.is_mute;
        for key in desired {
            if self.windows.iter().any(|window| window.key() == &key) {
                continue;
            }
            if self
                .policies
                .get(&key)
                .is_some_and(|policy| policy.waits_before_start(now))
            {
                continue;
            }
            let Some(monitor) = self
                .monitors
                .iter()
                .find(|monitor| monitor.name == key.monitor)
                .cloned()
            else {
                continue;
            };
            if key.kind == ContentKind::Video && Path::new(&key.source).is_file() {
                if let Some(index) = self.windows.iter().position(|window| {
                    window.key().kind == ContentKind::Video && window.key().monitor == key.monitor
                }) {
                    let switch_started = Instant::now();
                    match self.windows[index].load_video(&key.source, volume, muted) {
                        Ok(()) => {
                            log::info!(
                                "[Mode] {} video -> {} (reused player in {:?})",
                                key.monitor,
                                key.source,
                                switch_started.elapsed()
                            );
                            let policy = self
                                .policies
                                .entry(key.clone())
                                .or_insert_with(|| RestartPolicy::new(now));
                            policy.on_started(now);
                            continue;
                        }
                        Err(err) => {
                            log::warn!(
                                "[Mode] Reusing the player on {} failed: {err}",
                                key.monitor
                            );
                            self.windows.remove(index);
                        }
                    }
                }
            }
            log::info!(
                "[Mode] {} {} -> {}",
                key.monitor,
                kind_label(key.kind),
                key.source
            );
            match self.create_window(&key, &monitor) {
                Ok(window) => {
                    self.windows.push(window);
                    self.pending_icon_refresh = true;
                    // A new window starts paused. Drop the cached decision so
                    // `apply_playback_states` applies it even when the monitor
                    // already had the same play/pause state.
                    self.last_plans.remove(&key.monitor);
                    let policy = self
                        .policies
                        .entry(key.clone())
                        .or_insert_with(|| RestartPolicy::new(now));
                    policy.on_started(now);
                }
                Err(err) => self.notify_window_error(&key, err),
            }
        }

        // New windows have no entry in `last_plans`, so this only fades them
        // in/out. Existing players keep their current volume instead of
        // dipping to zero on every rebuild.
        self.apply_playback_states();
        self.apply_mute_states();
    }

    fn desired_windows(&self) -> Vec<WindowKey> {
        desired_windows_for(&self.config, &self.monitors, self.stream.as_ref())
    }

    fn create_window(&self, key: &WindowKey, monitor: &MonitorInfo) -> anyhow::Result<Engine> {
        let volume = self.config.volume;
        let muted = self.config.is_mute;
        let profile = self.config.playback_profile.as_str();
        let hwdec = self.config.hwdec.as_str();
        match key.kind {
            ContentKind::Video => Engine::video(monitor, &key.source, volume, muted, profile, hwdec),
            ContentKind::Stream => {
                let stream = self
                    .stream
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("stream is not resolved yet"))?;
                Engine::stream(monitor, stream, volume, muted, profile, hwdec)
            }
            ContentKind::Web => Engine::web(monitor, &key.source),
        }
    }

    fn notify_window_error(&self, key: &WindowKey, err: anyhow::Error) {
        let (kind, summary) = match key.kind {
            ContentKind::Video => {
                let name = Path::new(&key.source)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| key.source.clone());
                (ErrorKind::Playback, format!("Could not play {name}"))
            }
            ContentKind::Stream => (ErrorKind::Network, "Could not stream".to_string()),
            ContentKind::Web => (ErrorKind::Network, "Could not open webpage".to_string()),
        };
        if key.kind == ContentKind::Stream {
            // The signed URL came from yt-dlp and may have expired or been
            // revoked. Forget it so the next Apply or Reload asks the site
            // again instead of replaying the dead link until the TTL runs out.
            let page = self.config.default_source().to_string();
            if !page.is_empty() {
                ytdlp::invalidate(&page);
                log::info!("[Stream] Dropped the cached resolution after a playback error");
            }
        }
        self.notify_user_error(UserError::with_detail(kind, summary, err.to_string()));
    }

    fn finish_rebuild(&mut self) {
        self.update_static_wallpaper();
        // The extension reload exists to uncover a *newly mapped* wallpaper
        // window. Running it on every source switch made the desktop icons
        // blink several times a minute; a reused player needs nothing.
        if self.pending_icon_refresh && !self.windows.is_empty() {
            self.pending_icon_refresh = false;
            util::gnome_desktop_icon_workaround();
        }
        self.sync_snapshot();
    }

    /// Re-read `config.json` from disk (D-Bus `reload_config`).
    fn reload_config(&mut self) {
        let names: Vec<String> = self
            .monitors
            .iter()
            .map(|monitor| monitor.name.clone())
            .collect();
        self.config = Config::load(&names);
        self.idle_threshold.store(
            self.config.idle_timeout_sec.clamp(30, 86_400) as u64,
            Ordering::Relaxed,
        );
        // Volume, mute, mode and sources all come from the file now, so the
        // players are recreated rather than patched. Streaming sources are
        // page URLs and have to be resolved again.
        if self.config.mode == Mode::Stream {
            self.stream = None;
        }
        self.close_windows();
        self.rebuild_windows();
    }

    /// D-Bus `quit_player`: stop playback but keep the server (D-Bus, tray,
    /// config) alive while the player itself is stopped.
    fn quit_player(&mut self) {
        log::info!("[Player] Stopped by request");
        self.close_windows();
        self.sync_snapshot();
    }

    fn close_windows(&mut self) {
        self.windows.clear();
        self.policies.clear();
        self.last_plans.clear();
    }

    /// Poll every player once. A dead mpv is removed and recreated according to
    /// `RestartPolicy`; the policy prevents a broken file from spawning mpv in
    /// a tight loop.
    fn watch_players(&mut self) {
        let now = Instant::now();
        log::trace!("[Player] tick: {} window(s)", self.windows.len());
        // Command-level errors arrive asynchronously from the IPC worker.
        // Reporting them here keeps the owning thread free of IPC waits.
        let mut reported_errors = Vec::new();
        for window in &mut self.windows {
            if let Some(message) = window.take_error() {
                reported_errors.push((window.key().clone(), message));
            }
        }
        for (key, message) in reported_errors {
            self.notify_window_error(&key, anyhow::anyhow!(message));
        }

        let mut crashed = Vec::new();
        for window in &mut self.windows {
            if !window.player_alive() {
                crashed.push(window.key().clone());
            }
        }
        if !crashed.is_empty() {
            self.windows
                .retain(|window| !crashed.iter().any(|key| key == window.key()));
            for key in crashed {
                let outcome = {
                    let policy = self
                        .policies
                        .entry(key.clone())
                        .or_insert_with(|| RestartPolicy::new(now));
                    policy.on_exit(now)
                };
                match outcome {
                    Retry::Scheduled(delay) => {
                        log::warn!("[Player] {} stopped; retrying in {delay:?}", key.monitor)
                    }
                    Retry::GivenUp => {
                        log::error!(
                            "[Player] {} stopped {} times; giving up",
                            key.monitor,
                            MAX_RETRIES + 1
                        );
                        self.notify_window_error(
                            &key,
                            anyhow::anyhow!("playback stopped repeatedly"),
                        );
                        // The cached link is invalidated by `notify_window_error`;
                        // forget the resolved stream too, so the next rebuild
                        // resolves instead of recreating a window from it.
                        if key.kind == ContentKind::Stream {
                            self.stream = None;
                        }
                        // Stop the automatic loop. A later user action (Apply,
                        // Reload, monitor change) starts with a clean policy.
                        self.policies.remove(&key);
                    }
                }
            }
        }

        // Free decoders that have been paused out of sight for a while. The
        // decision is checked every second so a monitor that becomes visible
        // again is resumed through `play_fade` before its decoder is released.
        let allowed: Vec<bool> = self
            .windows
            .iter()
            .map(|window| self.parking_allowed(&window.key().monitor))
            .collect();
        for (window, allowed) in self.windows.iter_mut().zip(allowed) {
            window.set_park_allowed(allowed);
            window.park_if_due(now, DECODER_PARK_AFTER);
        }

        let desired = self.desired_windows();
        let pending_stream = self.config.mode == Mode::Stream && self.stream.is_none();
        if !pending_stream {
            self.policies.retain(|key, _| desired.contains(key));
        }
        let due: Vec<WindowKey> = self
            .policies
            .iter_mut()
            .filter_map(|(key, policy)| policy.retry_is_due(now).then(|| key.clone()))
            .collect();
        if due.is_empty() {
            return;
        }

        let volume = self.config.volume;
        let duration = self.config.fade_duration_sec;
        let interval = self.config.fade_interval;
        for key in due {
            if self.windows.iter().any(|window| window.key() == &key) || !desired.contains(&key) {
                continue;
            }
            let Some(monitor) = self
                .monitors
                .iter()
                .find(|monitor| monitor.name == key.monitor)
                .cloned()
            else {
                continue;
            };
            let plan = self.eco_plan_for(&key.monitor);
            match self.create_window(&key, &monitor) {
                Ok(mut window) => {
                    if plan.play {
                        window.play_fade(volume, duration, interval);
                    } else {
                        window.pause_fade(duration, interval);
                    }
                    window.set_fps_cap(plan.fps);
                    self.windows.push(window);
                    self.last_plans.insert(key.monitor.clone(), plan);
                    if let Some(policy) = self.policies.get_mut(&key) {
                        policy.on_started(now);
                    }
                    log::info!("[Player] restarted {} ({:?})", key.monitor, key.kind);
                }
                Err(err) => {
                    let outcome = {
                        let policy = self
                            .policies
                            .entry(key.clone())
                            .or_insert_with(|| RestartPolicy::new(now));
                        policy.on_exit(now)
                    };
                    match outcome {
                        Retry::Scheduled(delay) => log::warn!(
                            "[Player] restart of {} failed: {err}; retrying in {delay:?}",
                            key.monitor
                        ),
                        Retry::GivenUp => {
                            self.notify_window_error(&key, err);
                            self.policies.remove(&key);
                        }
                    }
                }
            }
        }
    }

    fn monitor_visibility(&self, monitor: &str) -> x11::MonitorVisibility {
        self.monitors
            .iter()
            .find(|info| info.name == monitor)
            .map(|info| {
                let (x, y, width, height) = monitor_rect(info);
                self.window_state.visibility_at(x, y, width, height)
            })
            .unwrap_or_default()
    }

    /// Evaluate the Eco Engine for one monitor. The same session-wide signals
    /// (lock, suspend, battery, idle) are combined with that monitor's own
    /// occlusion sample, so a game on HDMI-1 never pauses eDP-1.
    fn eco_plan_for(&self, monitor: &str) -> eco::Plan {
        let visibility = self.monitor_visibility(monitor);
        eco::plan(
            eco::Profile::from_config(&self.config.playback_profile),
            self.config.is_pause_when_maximized,
            eco::Signals {
                user_paused: self.paused_by_user,
                screen_locked: self.screen_locked,
                suspended: self.suspended,
                battery_paused: self.config.pause_on_battery && self.on_battery,
                idle: self.idle,
                covered: visibility.covered,
                fullscreen: visibility.fullscreen,
            },
        )
    }

    /// Parking releases the decoder by unloading the file. That is only safe
    /// while nobody can see the monitor: a lock screen, system suspend, or a
    /// window covering the wallpaper. A user pause leaves the last frame on
    /// screen, so it must keep the file loaded.
    fn parking_allowed(&self, monitor: &str) -> bool {
        if self.paused_by_user {
            return false;
        }
        if self.screen_locked || self.suspended {
            return true;
        }
        let visibility = self.monitor_visibility(monitor);
        visibility.covered || visibility.fullscreen
    }

    fn apply_playback_states(&mut self) {
        let plans: Vec<(String, eco::Plan, bool)> = self
            .windows
            .iter()
            .map(|window| {
                let monitor = window.key().monitor.clone();
                let plan = self.eco_plan_for(&monitor);
                let park_allowed = self.parking_allowed(&monitor);
                (monitor, plan, park_allowed)
            })
            .collect();
        let target = self.config.volume;
        let duration = self.config.fade_duration_sec;
        let interval = self.config.fade_interval;
        for (window, (monitor, plan, park_allowed)) in self.windows.iter_mut().zip(plans) {
            // A monitor that becomes visible must reload a parked file even
            // when the plan stays `Idle` (Eco still paused).
            window.set_park_allowed(park_allowed);
            if self.last_plans.get(&monitor) == Some(&plan) {
                continue;
            }
            log::debug!(
                "[Eco] {monitor}: {:?} (play={}, fps={:?})",
                plan.state,
                plan.play,
                plan.fps
            );
            if plan.play {
                window.play_fade(target, duration, interval);
            } else {
                window.pause_fade(duration, interval);
            }
            window.set_fps_cap(plan.fps);
            self.last_plans.insert(monitor, plan);
        }
        self.sync_snapshot();
    }

    /// Apply master mute and the per-monitor "mute when covered" rule. This
    /// also reaches Web wallpapers, which have no mpv volume.
    fn apply_mute_states(&mut self) {
        let master = self.config.is_mute;
        let mute_when_covered = self.config.is_mute_when_maximized;
        let target = self.config.volume;
        let duration = self.config.fade_duration_sec;
        let interval = self.config.fade_interval;
        let plans: Vec<(bool, bool)> = self
            .windows
            .iter()
            .map(|window| {
                let visibility = self.monitor_visibility(&window.key().monitor);
                let hidden = visibility.covered || visibility.fullscreen;
                (master || (mute_when_covered && hidden), window.is_primary())
            })
            .collect();
        for (window, (muted, primary)) in self.windows.iter_mut().zip(plans) {
            if primary {
                window.volume_fade(if muted { 0 } else { target }, duration, interval);
            }
            window.set_mute(muted);
        }
    }

    fn any_playing(&self) -> bool {
        self.windows
            .iter()
            .any(|window| self.eco_plan_for(&window.key().monitor).play)
    }

    fn eco_state_for(&self, monitor: &str) -> eco::State {
        self.eco_plan_for(monitor).state
    }

    fn monitor_states(&self) -> HashMap<String, eco::State> {
        self.monitors
            .iter()
            .map(|monitor| (monitor.name.clone(), self.eco_state_for(&monitor.name)))
            .collect()
    }

    fn pause_all(&mut self) {
        let duration = self.config.fade_duration_sec;
        let interval = self.config.fade_interval;
        for window in &mut self.windows {
            window.pause_fade(duration, interval);
        }
        self.last_plans.clear();
    }

    fn set_volume(&mut self, volume: i64) {
        self.config.volume = volume.clamp(0, 100);
        self.mark_config_dirty();
        let volume = self.config.volume;
        let plans: Vec<bool> = self
            .windows
            .iter()
            .map(|window| {
                let visibility = self.monitor_visibility(&window.key().monitor);
                !(visibility.covered || visibility.fullscreen)
            })
            .collect();
        for (window, visible) in self.windows.iter_mut().zip(plans) {
            if window.is_primary() && visible {
                window.set_volume(volume);
            }
        }
        self.apply_mute_states();
        self.sync_snapshot();
    }

    fn set_mute(&mut self, muted: bool) {
        self.config.is_mute = muted;
        self.save_config();
        self.apply_mute_states();
        self.sync_snapshot();
    }

    fn set_paused_by_user(&mut self, paused: bool) {
        self.paused_by_user = paused;
        self.apply_playback_states();
        self.sync_snapshot();
    }

    fn set_static_wallpaper(&mut self, enabled: bool) {
        self.config.is_static_wallpaper = enabled;
        self.save_config();
        self.update_static_wallpaper();
        self.sync_snapshot();
    }

    fn set_blur_radius(&mut self, radius: i64) {
        self.config.blur_radius = radius.clamp(0, 20);
        self.mark_config_dirty();
        if self.config.is_static_wallpaper && self.config.mode == Mode::Video {
            self.update_static_wallpaper();
        }
        self.sync_snapshot();
    }

    /// Advance every monitor whose rotation timer is due. Collections are
    /// chosen on the Home screen; the mode and interval are global settings.
    fn rotate_wallpapers(&mut self) {
        if self.config.rotation_mode == "off" {
            return;
        }
        let interval =
            Duration::from_secs(self.config.rotation_interval_sec.clamp(10, 86_400) as u64);
        let now = Instant::now();
        let mut changed = false;
        for monitor in self.monitors.clone() {
            let Some(collection_id) = self.config.rotations.get(&monitor.name).cloned() else {
                continue;
            };
            let items: Vec<String> = self
                .library
                .borrow()
                .collection(&collection_id)
                .map(|collection| {
                    collection
                        .items
                        .iter()
                        .filter(|item| Path::new(item).is_file())
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            if items.is_empty() {
                continue;
            }
            let last = self
                .last_rotation
                .entry(monitor.name.clone())
                .or_insert(now);
            if now.duration_since(*last) < interval {
                continue;
            }
            let current = self.config.source_for(&monitor.name).to_string();
            let Some(next) = next_rotation_path(&self.config.rotation_mode, &items, &current)
            else {
                continue;
            };
            if next == current {
                *last = now;
                continue;
            }
            self.config.mode = Mode::Video;
            self.config
                .data_source
                .insert(monitor.name.clone(), next.clone());
            if !Path::new(self.config.default_source()).is_file() {
                self.config.data_source.insert("Default".to_string(), next);
            }
            *last = now;
            changed = true;
        }
        if changed {
            self.save_config();
            self.sync_windows();
            self.finish_rebuild();
        }
    }

    fn update_static_wallpaper(&mut self) {
        let source = util::primary_source(&self.config, &self.monitors).to_string();
        let blur = self.config.blur_radius.max(0);
        let wanted = self.config.mode == Mode::Video
            && self.config.is_static_wallpaper
            && Path::new(&source).is_file()
            && self.background_wallpaper.is_supported();
        if !wanted {
            self.static_generation += 1;
            self.static_job = None;
            self.background_wallpaper.restore();
            return;
        }
        if self.static_job.as_ref() == Some(&(source.clone(), blur)) {
            return;
        }

        // ffmpeg can take seconds on 4K video; never run it under the event
        // loop. The generation counter discards a stale result if the source or
        // blur radius changes while the worker is running.
        self.static_generation += 1;
        let generation = self.static_generation;
        self.static_job = Some((source.clone(), blur));
        let tx = self.tx.clone();
        thread::Builder::new()
            .name("ferramari-static-frame".to_string())
            .spawn(move || {
                let file = crate::wallpaper::render_static_frame(Path::new(&source), blur as u32);
                let _ = tx.send(Command::StaticWallpaperReady {
                    source,
                    blur,
                    generation,
                    file,
                });
            })
            .ok();
    }

    /// The control surface is a separate binary now: ask the session to start
    /// it rather than opening a window in this process.
    pub fn show_gui(&mut self) {
        if let Err(err) = launch_control_surface() {
            log::warn!("[GUI] Could not start the control window: {err}");
        }
        if self.config.is_first_time {
            self.config.is_first_time = false;
            self.save_config();
        }
        self.gui_shown = true;
    }

    pub fn set_autostart(&mut self, enabled: bool) {
        if let Err(err) = autostart::set_enabled(enabled) {
            self.notify_user_error(UserError::with_detail(
                ErrorKind::External,
                "Could not update autostart",
                err.to_string(),
            ));
        }
        self.sync_snapshot();
    }

    fn quit(&mut self) {
        log::info!("[Server] Stopping");
        self.flush_config();
        self.quit_requested = true;
    }

    /// Coalesce rapid slider changes into one write.
    fn mark_config_dirty(&mut self) {
        self.config_dirty_at = Some(Instant::now());
    }

    fn flush_config_if_due(&mut self) {
        if self
            .config_dirty_at
            .is_some_and(|at| at.elapsed() >= CONFIG_FLUSH_AFTER)
        {
            self.flush_config();
        }
    }

    fn flush_config(&mut self) {
        if self.config_dirty_at.take().is_some() {
            self.save_config();
        }
    }

    fn save_config(&self) {
        log::debug!("[Config] Saving {}", paths::config_path().display());
        if let Err(err) = self.config.save() {
            self.notify_user_error(UserError::with_detail(
                ErrorKind::Config,
                "Could not save the configuration",
                err,
            ));
        }
    }

    fn sync_snapshot(&self) {
        let snapshot = Snapshot {
            mode: self.config.mode,
            volume: self.config.volume,
            blur_radius: self.config.blur_radius,
            is_mute: self.config.is_mute,
            is_playing: self.any_playing(),
            is_paused_by_user: self.paused_by_user,
            is_static_wallpaper: self.config.is_static_wallpaper,
            is_pause_when_maximized: self.config.is_pause_when_maximized,
            is_mute_when_maximized: self.config.is_mute_when_maximized,
            theme: self.config.theme.clone(),
            data_source: self.config.data_source.clone(),
            monitor_states: self
                .monitor_states()
                .into_iter()
                .map(|(monitor, state)| (monitor, state.as_str().to_string()))
                .collect(),
            language: self.config.language.clone(),
            rotation_mode: self.config.rotation_mode.clone(),
            rotation_interval_sec: self.config.rotation_interval_sec,
            playback_profile: self.config.playback_profile.clone(),
            pause_on_battery: self.config.pause_on_battery,
            idle_timeout_sec: self.config.idle_timeout_sec,
            hwdec: self.config.hwdec.clone(),
            web_allow_network: self.config.web_allow_network,
            fade_duration_sec: self.config.fade_duration_sec,
            fade_interval: self.config.fade_interval,
            is_show_systray: self.config.is_show_systray,
            autostart_enabled: autostart::is_enabled(),
            rotations: self.config.rotations.clone(),
        };
        if let Ok(mut shared) = self.snapshot.lock() {
            *shared = snapshot;
        }
    }

    fn notify_user_error(&self, error: UserError) {
        error.log();
    }
}

/// Start the Iced control surface next to this executable. It is a normal
/// D-Bus client, so the server never has to know about its widgets.
fn launch_control_surface() -> anyhow::Result<()> {
    let exe = std::env::current_exe()?;
    let dir = exe
        .parent()
        .ok_or_else(|| anyhow::anyhow!("executable has no directory"))?;
    let candidate = dir.join("ferramari-ui");
    let program = if candidate.is_file() {
        candidate
    } else {
        PathBuf::from("ferramari-ui")
    };
    // A second `show_gui` while one window is open must not stack windows;
    // `ferramari-ui` exits immediately when another instance owns its name.
    // Spawning a process is quick and returning the error here lets
    // show_gui() report a missing or non-executable UI binary to the log.
    std::process::Command::new(&program)
        .spawn()
        .map(|_| ())
        .map_err(|err| {
            anyhow::anyhow!(
                "could not launch control surface {}: {err}",
                program.display()
            )
        })
}

/// Pure planning step: which wallpaper window should exist for every monitor
/// in the current mode. Keeping it free of windowing makes hotplug and monitor
/// reattachment behaviour testable.
fn desired_windows_for(
    config: &Config,
    monitors: &[MonitorInfo],
    stream: Option<&ytdlp::StreamInfo>,
) -> Vec<WindowKey> {
    match config.mode {
        Mode::Video => monitors
            .iter()
            .filter_map(|monitor| {
                let source = config.source_for(&monitor.name);
                (!source.is_empty()).then(|| WindowKey {
                    kind: ContentKind::Video,
                    monitor: monitor.name.clone(),
                    source: source.to_string(),
                })
            })
            .collect(),
        Mode::Stream => match stream {
            Some(stream) => monitors
                .iter()
                .map(|monitor| WindowKey {
                    kind: ContentKind::Stream,
                    monitor: monitor.name.clone(),
                    source: stream.video_url.clone(),
                })
                .collect(),
            None => Vec::new(),
        },
        Mode::Webpage => {
            let uri = config.default_source();
            if uri.is_empty() {
                Vec::new()
            } else {
                monitors
                    .iter()
                    .map(|monitor| WindowKey {
                        kind: ContentKind::Web,
                        monitor: monitor.name.clone(),
                        source: uri.to_string(),
                    })
                    .collect()
            }
        }
        Mode::Null | Mode::Unknown => Vec::new(),
    }
}

/// X11 root coordinates are device pixels; monitor geometry from RandR is
/// logical pixels on HiDPI/XWayland layouts. Convert once so the visibility
/// check and its tests use the same rectangle.
fn monitor_rect(info: &MonitorInfo) -> (i32, i32, i32, i32) {
    let scale = info.scale_factor.max(1);
    (
        info.x.saturating_mul(scale),
        info.y.saturating_mul(scale),
        info.width.saturating_mul(scale),
        info.height.saturating_mul(scale),
    )
}

/// A local page may arrive as a plain path or as a `file://` URI (for example
/// from a D-Bus caller). Percent escapes are decoded and a non-local authority
/// is refused, so the result is always a path on this machine.
fn local_webpage_path(url: &str) -> Option<PathBuf> {
    util::uri_to_path(url)
}

/// Apply a local video to one monitor or all of them. When switching away from
/// web/stream mode, a Default URL must not be left for monitors without their
/// own video: it would be handed to mpv and fail. Fill that gap with the new
/// video instead.
fn assign_video(
    config: &mut Config,
    monitors: &[MonitorInfo],
    path: String,
    monitor: Option<&str>,
) -> bool {
    if let Some(name) = monitor {
        if name != "all" && !monitors.iter().any(|info| info.name == name) {
            // A D-Bus caller can name anything. Writing an unknown key would
            // also rewrite Default and change every monitor without a source.
            log::warn!("[Video] Unknown monitor {name:?}, ignoring");
            return false;
        }
    }
    let previous_mode = config.mode;
    config.mode = Mode::Video;
    match monitor {
        Some("all") | None => {
            for info in monitors {
                config.data_source.insert(info.name.clone(), path.clone());
            }
            config.data_source.insert("Default".to_string(), path);
        }
        Some(name) => {
            config.data_source.insert(name.to_string(), path.clone());
            if previous_mode != Mode::Video {
                // A stream or webpage URL is not a local-video fallback for
                // the other monitors. Remove it so only the selected monitor
                // receives the newly assigned video.
                config.data_source.remove("Default");
            } else {
                let default = config.default_source().to_string();
                if !Path::new(&default).is_file() {
                    config.data_source.insert("Default".to_string(), path);
                }
            }
        }
    }
    true
}

/// Pick the next collection item. Order mode advances from the current file;
/// random mode avoids repeating it when there is more than one candidate.
fn next_rotation_path(mode: &str, items: &[String], current: &str) -> Option<String> {
    if items.is_empty() {
        return None;
    }
    if mode == "random" {
        let candidates: Vec<&String> = items
            .iter()
            .filter(|item| item.as_str() != current)
            .collect();
        if candidates.is_empty() {
            return items.first().cloned();
        }
        return candidates
            .choose(&mut rand::thread_rng())
            .map(|item| (*item).clone());
    }
    let index = items
        .iter()
        .position(|item| item == current)
        .map(|index| (index + 1) % items.len())
        .unwrap_or(0);
    items.get(index).cloned()
}

fn kind_label(kind: ContentKind) -> &'static str {
    match kind {
        ContentKind::Video => "video",
        ContentKind::Stream => "stream",
        ContentKind::Web => "web page",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor(name: &str, x: i32) -> MonitorInfo {
        MonitorInfo {
            index: 0,
            name: name.to_string(),
            x,
            y: 0,
            width: 1920,
            height: 1080,
            scale_factor: 1,
            is_primary: name == "eDP-1",
        }
    }

    #[test]
    fn removing_and_reattaching_a_monitor_keeps_its_source() {
        let monitors = vec![monitor("eDP-1", 0), monitor("HDMI-1", 1920)];
        let mut config = Config::template(
            &monitors
                .iter()
                .map(|monitor| monitor.name.clone())
                .collect::<Vec<_>>(),
        );
        config.mode = Mode::Video;
        for monitor in &monitors {
            config
                .data_source
                .insert(monitor.name.clone(), format!("/videos/{}.mp4", monitor.name));
        }

        // Unplug HDMI-1: only the built-in display should get a window.
        let attached = vec![monitor("eDP-1", 0)];
        let wanted = desired_windows_for(&config, &attached, None);
        assert_eq!(wanted.len(), 1);
        assert_eq!(wanted[0].monitor, "eDP-1");

        // Plug it back in: its own source is still in the config, so the
        // window comes back with the same file.
        let wanted = desired_windows_for(&config, &monitors, None);
        assert_eq!(wanted.len(), 2);
        let hdmi = wanted
            .iter()
            .find(|key| key.monitor == "HDMI-1")
            .expect("HDMI-1 window");
        assert_eq!(hdmi.source, "/videos/HDMI-1.mp4");
    }

    #[test]
    fn assigning_one_monitor_from_web_mode_does_not_leave_a_url_for_mpv() {
        let monitors = vec![monitor("eDP-1", 0), monitor("HDMI-1", 1920)];
        let mut config = Config::template(&["eDP-1".to_string(), "HDMI-1".to_string()]);
        config.mode = Mode::Webpage;
        config
            .data_source
            .insert("Default".to_string(), "https://example.test".to_string());

        assert!(assign_video(
            &mut config,
            &monitors,
            "/videos/a.mp4".to_string(),
            Some("eDP-1"),
        ));
        // HDMI-1 has no video of its own; leaving the web URL in Default would
        // hand it to mpv as a file path.
        let wanted = desired_windows_for(&config, &monitors, None);
        assert_eq!(wanted.len(), 1);
        assert_eq!(wanted[0].monitor, "eDP-1");
    }

    #[test]
    fn assigning_to_an_unknown_monitor_is_rejected() {
        let monitors = vec![monitor("eDP-1", 0)];
        let mut config = Config::template(&["eDP-1".to_string()]);
        assert!(!assign_video(
            &mut config,
            &monitors,
            "/videos/a.mp4".to_string(),
            Some("nope-1"),
        ));
    }

    #[test]
    fn order_rotation_advances_and_wraps() {
        let items = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        assert_eq!(next_rotation_path("order", &items, "a"), Some("b".to_string()));
        assert_eq!(next_rotation_path("order", &items, "c"), Some("a".to_string()));
        assert_eq!(next_rotation_path("order", &items, "gone"), Some("a".to_string()));
    }

    #[test]
    fn random_rotation_avoids_the_current_item() {
        let items = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        for _ in 0..50 {
            assert_ne!(next_rotation_path("random", &items, "a").as_deref(), Some("a"));
        }
        // One candidate left: it has to repeat.
        assert_eq!(
            next_rotation_path("random", &["a".to_string()], "a"),
            Some("a".to_string())
        );
    }

    #[test]
    fn hidpi_monitor_geometry_is_converted_to_device_pixels() {
        let mut info = monitor("eDP-1", 0);
        info.scale_factor = 2;
        assert_eq!(monitor_rect(&info), (0, 0, 3840, 2160));
    }

    #[test]
    fn local_webpages_accept_paths_and_file_uris() {
        assert_eq!(
            local_webpage_path("/tmp/a.html"),
            Some(PathBuf::from("/tmp/a.html"))
        );
        assert_eq!(
            local_webpage_path("file:///tmp/a%20b.html"),
            Some(PathBuf::from("/tmp/a b.html"))
        );
        assert_eq!(local_webpage_path("file://evil.example/a.html"), None);
    }

    #[test]
    fn a_monitor_without_a_source_gets_no_window() {
        let monitors = vec![monitor("eDP-1", 0), monitor("HDMI-1", 1920)];
        let mut config = Config::template(&["eDP-1".to_string(), "HDMI-1".to_string()]);
        config.mode = Mode::Video;
        config
            .data_source
            .insert("eDP-1".to_string(), "/videos/a.mp4".to_string());
        let wanted = desired_windows_for(&config, &monitors, None);
        assert_eq!(wanted.len(), 1);
        assert_eq!(wanted[0].monitor, "eDP-1");
    }
}
