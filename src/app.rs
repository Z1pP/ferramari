//! Application state and the command loop.
//!
//! Every part of the program that wants to change something (GUI callbacks,
//! D-Bus methods, X11/session watchers, tray) sends a [`Command`] into one
//! channel. The GTK main thread drains it and mutates [`App`], so GTK objects
//! are only ever touched from the thread that owns them and state changes are
//! serialised in one place.

use std::cell::RefCell;
use std::path::Path;
use std::rc::{Rc, Weak};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gtk::prelude::*;

use crate::autostart;
use crate::config::{Config, Mode};
use crate::handlers;
use crate::monitors::{self, MonitorInfo};
use crate::paths;
use crate::tray;
use crate::ui::control::ControlWindow;
use crate::ui::wallpaper::{WallpaperWindow, WindowKey, WindowKind};
use crate::util;
use crate::wallpaper::GnomeBackground;
use crate::x11::{self, WindowState};
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
    WindowState(WindowState),
    MonitorsChanged,
    SetAutostart(bool),
    SetLanguage(String),
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
    pub data_source: std::collections::HashMap<String, String>,
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
            data_source: std::collections::HashMap::new(),
        }
    }
}

pub struct App {
    gtk_app: gtk::Application,
    pub config: Config,
    pub monitors: Vec<MonitorInfo>,
    pub windows: Vec<WallpaperWindow>,
    pub control: Option<ControlWindow>,
    pub snapshot: Arc<Mutex<Snapshot>>,
    tx: Sender<Command>,
    weak_self: Weak<RefCell<App>>,
    background: bool,
    version: String,
    paused_by_user: bool,
    screen_locked: bool,
    is_any_maximized: bool,
    is_any_fullscreen: bool,
    stream: Option<ytdlp::StreamInfo>,
    stream_resolving: bool,
    background_wallpaper: GnomeBackground,
    window_watcher: Option<x11::Watcher>,
    tray_handle: Option<ksni::blocking::Handle<tray::Tray>>,
    quit_requested: bool,
    gui_shown: bool,
    hold_guard: Option<gtk::gio::ApplicationHoldGuard>,
}

impl App {
    pub fn new(
        gtk_app: &gtk::Application,
        args: &crate::cli::Args,
        tx: Sender<Command>,
        rx: Receiver<Command>,
        snapshot: Arc<Mutex<Snapshot>>,
    ) -> Rc<RefCell<Self>> {
        paths::ensure_dirs();
        let monitors = monitors::list();
        let names: Vec<String> = monitors
            .iter()
            .map(|monitor| monitor.name.clone())
            .collect();
        let config = Config::load(&names);
        let background_wallpaper = GnomeBackground::capture();

        let mut app = Self {
            gtk_app: gtk_app.clone(),
            config,
            monitors,
            windows: Vec::new(),
            control: None,
            snapshot,
            tx: tx.clone(),
            weak_self: Weak::new(),
            background: args.background,
            version: env!("CARGO_PKG_VERSION").to_string(),
            paused_by_user: false,
            screen_locked: false,
            is_any_maximized: false,
            is_any_fullscreen: false,
            stream: None,
            stream_resolving: false,
            background_wallpaper,
            window_watcher: None,
            tray_handle: None,
            quit_requested: false,
            gui_shown: false,
            hold_guard: None,
        };
        app.sync_snapshot();

        if app.config.is_show_systray {
            app.tray_handle = tray::spawn(tx.clone());
        }

        let app = Rc::new(RefCell::new(app));
        app.borrow_mut().weak_self = Rc::downgrade(&app);

        // Keep the process alive even when there is no wallpaper window and
        // the control window is hidden (upstream's background server).
        app.borrow_mut().hold_guard = Some(gtk_app.hold());

        install_actions(gtk_app, &tx);
        Self::start_command_loop(&app, rx);
        Self::start_external_watchers(&app);
        app
    }

    fn start_command_loop(app: &Rc<RefCell<Self>>, rx: Receiver<Command>) {
        let weak = Rc::downgrade(app);
        glib::timeout_add_local(Duration::from_millis(40), move || {
            let Some(app) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            loop {
                match rx.try_recv() {
                    Ok(command) => {
                        let mut state = app.borrow_mut();
                        state.handle(command);
                        if state.quit_requested {
                            return glib::ControlFlow::Break;
                        }
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => return glib::ControlFlow::Break,
                }
            }
            glib::ControlFlow::Continue
        });
    }

    fn start_external_watchers(app: &Rc<RefCell<Self>>) {
        let (tx, is_wayland) = {
            let app = app.borrow();
            (app.tx.clone(), paths::is_wayland())
        };

        handlers::start_screen_lock_watcher(tx.clone());
        handlers::start_end_session_watcher(tx.clone());
        if !is_wayland {
            if let Some(watcher) = x11::start(tx.clone()) {
                app.borrow_mut().window_watcher = Some(watcher);
            }
        }

        let Some(display) = gtk::gdk::Display::default() else {
            return;
        };
        let monitor_tx = tx.clone();
        display.connect_monitor_added(move |_, _| {
            let _ = monitor_tx.send(Command::MonitorsChanged);
        });
        let monitor_tx = tx.clone();
        display.connect_monitor_removed(move |_, _| {
            let _ = monitor_tx.send(Command::MonitorsChanged);
        });
        if let Some(screen) = gtk::gdk::Screen::default() {
            let screen_tx = tx;
            screen.connect_size_changed(move |_| {
                let _ = screen_tx.send(Command::MonitorsChanged);
            });
        }
    }

    pub fn activate(&mut self) {
        self.rebuild_windows();
        if !self.background {
            self.show_gui();
        }
    }

    pub fn shutdown(&mut self) {
        self.close_windows();
        self.background_wallpaper.restore();
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
                if let Some(control) = &self.control {
                    control.set_streaming(false);
                }
                match result {
                    Ok(info) => {
                        log::info!("[Stream] {}x{} from {url}", info.width, info.height);
                        self.stream = Some(info);
                        self.config.mode = Mode::Stream;
                        self.config.data_source.insert("Default".to_string(), url);
                        self.save_config();
                        self.rebuild_windows();
                    }
                    Err(err) => {
                        self.stream = None;
                        self.notify_error(err);
                        self.refresh_ui();
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
                Err(err) => self.notify_error(err),
            },
            Command::PausePlayback => {
                self.pause_all();
                self.sync_snapshot();
            }
            Command::StartPlayback => {
                if self.should_play() {
                    self.play_all();
                }
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
                self.refresh_ui();
                if self.config.is_pause_when_maximized {
                    if self.should_play() {
                        self.play_all();
                    } else {
                        self.pause_all();
                    }
                }
            }
            Command::SetMuteWhenMaximized(enabled) => {
                self.config.is_mute_when_maximized = enabled;
                self.save_config();
                self.refresh_ui();
                self.apply_maximized_mute();
            }
            Command::ScreenLockActive(active) => {
                log::debug!("[ScreenLock] active={active}");
                self.screen_locked = active;
                if active {
                    self.pause_all();
                } else if self.should_play() {
                    self.play_all();
                } else {
                    self.pause_all();
                }
                self.sync_snapshot();
            }
            Command::WindowState(state) => {
                let changed = state.is_any_maximized != self.is_any_maximized
                    || state.is_any_fullscreen != self.is_any_fullscreen;
                if !changed {
                    return;
                }
                self.is_any_maximized = state.is_any_maximized;
                self.is_any_fullscreen = state.is_any_fullscreen;
                log::info!(
                    "[WindowState] maximized={} fullscreen={}",
                    self.is_any_maximized,
                    self.is_any_fullscreen
                );
                if self.config.is_pause_when_maximized {
                    if self.should_play() {
                        self.play_all();
                    } else {
                        self.pause_all();
                    }
                } else if self.config.is_mute_when_maximized {
                    self.apply_maximized_mute();
                }
                self.sync_snapshot();
            }
            Command::SetAutostart(enabled) => self.set_autostart(enabled),
            Command::SetLanguage(language) => {
                if self.config.language != language {
                    self.config.language = language;
                    self.save_config();
                }
            }
            Command::MonitorsChanged => {
                log::info!("[Monitors] Layout changed");
                self.monitors = monitors::list();
                let names: Vec<String> = self
                    .monitors
                    .iter()
                    .map(|monitor| monitor.name.clone())
                    .collect();
                self.config = Config::load(&names);
                self.rebuild_windows();
            }
        }
        self.sync_snapshot();
    }

    fn apply_video(&mut self, path: Option<String>, monitor: Option<String>) {
        if let Some(path) = path {
            self.config.mode = Mode::Video;
            match monitor.as_deref() {
                Some("all") | None => {
                    for monitor in &self.monitors {
                        self.config
                            .data_source
                            .insert(monitor.name.clone(), path.clone());
                    }
                    self.config.data_source.insert("Default".to_string(), path);
                }
                Some(monitor) => {
                    if !self.monitors.iter().any(|info| info.name == monitor) {
                        // A D-Bus caller can name anything. Writing an unknown
                        // key would also rewrite Default and change every
                        // monitor that has no source of its own.
                        log::warn!("[Video] Unknown monitor {monitor:?}, ignoring");
                        return;
                    }
                    self.config.data_source.insert(monitor.to_string(), path);
                }
            }
            self.save_config();
        }
        self.rebuild_windows();
    }

    /// Resolve a stream URL in the background. The mode only changes once a
    /// playable URL is known, so a bad link leaves the current wallpaper alone.
    fn resolve_stream(&mut self, url: String) {
        if self.stream_resolving {
            return;
        }
        self.stream_resolving = true;
        if let Some(control) = &self.control {
            control.set_streaming(true);
        }
        let screen_height = self
            .monitors
            .iter()
            .map(|monitor| monitor.height.max(0) as u32)
            .max()
            .unwrap_or(1080);
        let tx = self.tx.clone();
        log::info!("[Stream] Resolving: {url}");
        std::thread::spawn(move || {
            let result = ytdlp::resolve(&url, screen_height);
            let _ = tx.send(Command::StreamReady { url, result });
        });
    }

    fn start_webpage(&mut self, url: String) {
        if url.is_empty() {
            return;
        }
        if url.starts_with("http://") || url.starts_with("https://") || url.starts_with("file://") {
            let target = url.clone();
            let tx = self.tx.clone();
            std::thread::spawn(move || {
                let result = ytdlp::check_url(&target);
                let _ = tx.send(Command::WebpageReady {
                    url: target,
                    result,
                });
            });
            return;
        }
        let path = Path::new(&url);
        if !path.is_file() {
            self.notify_error(format!("File not found: {url}"));
            return;
        }
        self.config.mode = Mode::Webpage;
        self.config
            .data_source
            .insert("Default".to_string(), util::path_to_uri(path));
        self.save_config();
        self.rebuild_windows();
    }

    fn feeling_lucky(&mut self) {
        let videos = util::video_paths();
        let Some(video) = util::random_other_video(&videos, self.config.default_source())
            .or_else(|| videos.first().cloned())
        else {
            self.notify_error("No videos found in the Ferramari folder");
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
                self.resolve_stream(url);
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
        let desired = self.desired_windows();
        // While a stream is being re-resolved the old windows have no
        // replacement yet; keep them playing rather than showing a black
        // desktop, then swap once `StreamReady` arrives.
        let pending_stream = self.config.mode == Mode::Stream && self.stream.is_none();
        self.windows.retain(|window| {
            desired.contains(window.key())
                || (pending_stream && window.key().kind == WindowKind::Stream)
        });

        let mut created = Vec::new();
        for key in desired {
            if self.windows.iter().any(|window| window.key() == &key) {
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
            log::info!(
                "[Mode] {} {} -> {}",
                key.monitor,
                kind_label(key.kind),
                key.source
            );
            match self.create_window(&key, &monitor) {
                Ok(window) => {
                    self.windows.push(window);
                    created.push(self.windows.len() - 1);
                }
                Err(err) => self.notify_window_error(&key, err),
            }
        }

        // Only new windows fade in; a player that is already running keeps its
        // current volume instead of dipping to zero on every rebuild.
        let volume = self.config.volume;
        let duration = self.config.fade_duration_sec;
        let interval = self.config.fade_interval;
        let should_play = self.should_play();
        for index in created {
            let window = &mut self.windows[index];
            if should_play {
                window.play_fade(volume, duration, interval);
            } else {
                window.pause_fade(duration, interval);
            }
        }
    }

    fn desired_windows(&self) -> Vec<WindowKey> {
        match self.config.mode {
            Mode::Video => self
                .monitors
                .iter()
                .filter_map(|monitor| {
                    let source = self.config.source_for(&monitor.name);
                    (!source.is_empty()).then(|| WindowKey {
                        kind: WindowKind::Video,
                        monitor: monitor.name.clone(),
                        source: source.to_string(),
                    })
                })
                .collect(),
            Mode::Stream => match &self.stream {
                Some(stream) => self
                    .monitors
                    .iter()
                    .map(|monitor| WindowKey {
                        kind: WindowKind::Stream,
                        monitor: monitor.name.clone(),
                        source: stream.video_url.clone(),
                    })
                    .collect(),
                None => Vec::new(),
            },
            Mode::Webpage => {
                let uri = self.config.default_source();
                if uri.is_empty() {
                    log::info!("[Mode] No webpage configured");
                    Vec::new()
                } else {
                    self.monitors
                        .iter()
                        .map(|monitor| WindowKey {
                            kind: WindowKind::Web,
                            monitor: monitor.name.clone(),
                            source: uri.to_string(),
                        })
                        .collect()
                }
            }
            Mode::Null | Mode::Unknown => Vec::new(),
        }
    }

    fn create_window(
        &self,
        key: &WindowKey,
        monitor: &MonitorInfo,
    ) -> anyhow::Result<WallpaperWindow> {
        let volume = self.config.volume;
        let muted = self.config.is_mute;
        match key.kind {
            WindowKind::Video => {
                WallpaperWindow::video(&self.gtk_app, monitor, &key.source, volume, muted)
            }
            WindowKind::Stream => {
                let stream = self
                    .stream
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("stream is not resolved yet"))?;
                WallpaperWindow::stream(&self.gtk_app, monitor, stream, volume, muted)
            }
            WindowKind::Web => WallpaperWindow::web(&self.gtk_app, monitor, &key.source, muted),
        }
    }

    fn notify_window_error(&self, key: &WindowKey, err: anyhow::Error) {
        match key.kind {
            WindowKind::Video => {
                let name = Path::new(&key.source)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| key.source.clone());
                self.notify_error(format!("Could not play {name}: {err}"));
            }
            WindowKind::Stream => self.notify_error(format!("Could not stream: {err}")),
            WindowKind::Web => self.notify_error(format!("Could not open webpage: {err}")),
        }
    }

    fn finish_rebuild(&mut self) {
        self.update_static_wallpaper();
        util::gnome_desktop_icon_workaround();
        self.sync_snapshot();
        self.refresh_ui();
    }

    /// Re-read `config.json` from disk (D-Bus `reload_config`).
    fn reload_config(&mut self) {
        let names: Vec<String> = self
            .monitors
            .iter()
            .map(|monitor| monitor.name.clone())
            .collect();
        self.config = Config::load(&names);
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
    /// config) alive, matching upstream's separate player process.
    fn quit_player(&mut self) {
        log::info!("[Player] Stopped by request");
        self.close_windows();
        self.sync_snapshot();
        self.refresh_ui();
    }

    fn close_windows(&mut self) {
        self.windows.clear();
    }

    fn should_play(&self) -> bool {
        if self.screen_locked {
            return false;
        }
        if self.config.is_pause_when_maximized && (self.is_any_maximized || self.is_any_fullscreen)
        {
            return false;
        }
        !self.paused_by_user
    }

    fn play_all(&mut self) {
        if !self.should_play() {
            return;
        }
        let target = self.config.volume;
        let duration = self.config.fade_duration_sec;
        let interval = self.config.fade_interval;
        for window in &mut self.windows {
            window.play_fade(target, duration, interval);
        }
    }

    fn pause_all(&mut self) {
        let duration = self.config.fade_duration_sec;
        let interval = self.config.fade_interval;
        for window in &mut self.windows {
            window.pause_fade(duration, interval);
        }
    }

    fn apply_maximized_mute(&mut self) {
        let target = if self.is_any_maximized || self.is_any_fullscreen {
            0
        } else {
            self.config.volume
        };
        let duration = self.config.fade_duration_sec;
        let interval = self.config.fade_interval;
        for window in &mut self.windows {
            if window.is_primary() {
                window.volume_fade(target, duration, interval);
            }
        }
    }

    fn set_volume(&mut self, volume: i64) {
        self.config.volume = volume.clamp(0, 100);
        self.save_config();
        let volume = self.config.volume;
        for window in &mut self.windows {
            if window.is_primary() {
                window.set_volume(volume);
            }
        }
        self.sync_snapshot();
        self.refresh_ui();
    }

    fn set_mute(&mut self, muted: bool) {
        self.config.is_mute = muted;
        self.save_config();
        for window in &mut self.windows {
            if window.is_primary() {
                window.set_mute(muted);
            }
        }
        self.sync_snapshot();
        self.refresh_ui();
    }

    fn set_paused_by_user(&mut self, paused: bool) {
        self.paused_by_user = paused;
        if paused {
            self.pause_all();
        } else if self.should_play() {
            self.play_all();
        }
        self.sync_snapshot();
        self.refresh_ui();
    }

    fn set_static_wallpaper(&mut self, enabled: bool) {
        self.config.is_static_wallpaper = enabled;
        self.save_config();
        self.update_static_wallpaper();
        self.sync_snapshot();
        self.refresh_ui();
    }

    fn set_blur_radius(&mut self, radius: i64) {
        self.config.blur_radius = radius.clamp(0, 20);
        self.save_config();
        if self.config.is_static_wallpaper && self.config.mode == Mode::Video {
            self.update_static_wallpaper();
        }
        self.sync_snapshot();
        self.refresh_ui();
    }

    fn update_static_wallpaper(&mut self) {
        let source = self.config.default_source().to_string();
        if self.config.mode == Mode::Video
            && self.config.is_static_wallpaper
            && Path::new(&source).is_file()
        {
            self.background_wallpaper
                .apply_static(Path::new(&source), self.config.blur_radius.max(0) as u32);
        } else {
            self.background_wallpaper.restore();
        }
    }

    pub fn show_gui(&mut self) {
        if self.control.is_none() {
            self.control = Some(ControlWindow::new(
                &self.gtk_app,
                self.tx.clone(),
                &self.version,
            ));
        }
        let first_time = self.config.is_first_time;
        let is_playing = self.should_play() && !self.windows.is_empty();
        if let Some(control) = self.control.as_ref() {
            control.present();
            control.refresh(
                &self.config,
                &self.monitors,
                autostart::is_enabled(),
                self.paused_by_user,
                is_playing,
            );
            if !self.gui_shown && first_time {
                control.show_welcome();
            }
        }
        if !self.gui_shown && first_time {
            self.config.is_first_time = false;
            self.save_config();
        }
        self.gui_shown = true;
    }

    pub fn set_autostart(&mut self, enabled: bool) {
        if let Err(err) = autostart::set_enabled(enabled) {
            self.notify_error(format!("Could not update autostart: {err}"));
        }
        self.refresh_ui();
    }

    fn quit(&mut self) {
        log::info!("[Server] Stopping");
        self.quit_requested = true;
        self.close_windows();
        self.background_wallpaper.restore();
        self.window_watcher = None;
        self.tray_handle = None;
        self.hold_guard = None;
        self.gtk_app.quit();
    }

    fn save_config(&self) {
        self.config.save();
    }

    fn sync_snapshot(&self) {
        let snapshot = Snapshot {
            mode: self.config.mode,
            volume: self.config.volume,
            blur_radius: self.config.blur_radius,
            is_mute: self.config.is_mute,
            is_playing: self.should_play() && !self.windows.is_empty(),
            is_paused_by_user: self.paused_by_user,
            is_static_wallpaper: self.config.is_static_wallpaper,
            is_pause_when_maximized: self.config.is_pause_when_maximized,
            is_mute_when_maximized: self.config.is_mute_when_maximized,
            data_source: self.config.data_source.clone(),
        };
        if let Ok(mut shared) = self.snapshot.lock() {
            *shared = snapshot;
        }
    }

    fn refresh_ui(&self) {
        if let Some(control) = &self.control {
            let is_playing = self.should_play() && !self.windows.is_empty();
            control.refresh(
                &self.config,
                &self.monitors,
                autostart::is_enabled(),
                self.paused_by_user,
                is_playing,
            );
        }
    }

    fn notify_error(&self, message: impl Into<String>) {
        let message = message.into();
        log::error!("{message}");
        if let Some(control) = &self.control {
            control.show_error(&message);
        }
    }
}

fn kind_label(kind: WindowKind) -> &'static str {
    match kind {
        WindowKind::Video => "video",
        WindowKind::Stream => "stream",
        WindowKind::Web => "web page",
    }
}

fn install_actions(app: &gtk::Application, tx: &Sender<Command>) {
    let actions = [
        ("show", Command::ShowGui),
        ("toggle-mute", Command::ToggleMute),
        ("toggle-pause", Command::TogglePause),
        ("reload", Command::Reload),
        ("lucky", Command::FeelingLucky),
        ("quit", Command::Quit),
    ];
    for (name, command) in actions {
        let action = gtk::gio::SimpleAction::new(name, None);
        let tx = tx.clone();
        action.connect_activate(move |_, _| {
            let _ = tx.send(command.clone());
        });
        app.add_action(&action);
    }
    app.set_accels_for_action("app.quit", &["<Primary>q"]);
}
