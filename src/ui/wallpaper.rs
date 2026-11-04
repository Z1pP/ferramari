//! Wallpaper windows: one X11 toplevel per monitor.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use webkit2gtk::{
    NavigationPolicyDecisionExt, PermissionRequestExt, PolicyDecisionExt, SettingsExt,
    URIRequestExt, WebViewExt,
};
use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::ConnectionExt as _;

use crate::app::{Command, CommandSender};
use crate::fade::Fade;
use crate::monitors::MonitorInfo;
use crate::mpv::{Mpv, Options as MpvOptions};
use crate::ui::menu;
use crate::ytdlp::StreamInfo;

pub struct WallpaperWindow {
    window: gtk::ApplicationWindow,
    content: Content,
    fade: Fade,
    primary: bool,
    /// Master/user mute state. `pause_fade` temporarily mutes a WebView while
    /// it is hidden; `play_fade` restores this value on resume.
    muted: bool,
    /// The profile's default `vf` chain, restored when an idle FPS cap ends.
    base_filter: String,
    /// When this wallpaper started its current pause, if it is paused.
    /// `park_if_due` uses it to release the decoder only after a long pause.
    paused_at: Option<Instant>,
    /// True once the embedded player released its file. The next `play_fade`
    /// reloads from the saved position instead of just clearing `pause`.
    parked: bool,
    /// The timer that applies `set_pause(true)` when a pause fade ends.
    ///
    /// A window has one fade slot, which the mute rule and the volume slider
    /// also use. Applying the pause at the end of that fade meant any later
    /// volume change cancelled it and the player kept decoding at volume 0.
    /// The pause therefore fades on its own slot, cancelled by `play_fade`.
    pause_timer: Fade,
    /// Whether the current pause is invisible to the user. Parking is only
    /// attempted while this is true, and switching it off reloads a parked
    /// file so a visible monitor never shows an empty surface.
    park_allowed: bool,
    key: WindowKey,
    // Kept alive for the lifetime of the window.
    _menu: gtk::Menu,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WindowKind {
    Video,
    Stream,
    Web,
}

/// What a wallpaper window shows: kind, monitor and source. Two windows are
/// interchangeable only when all three match, which lets a change on one
/// monitor keep the other players running.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WindowKey {
    pub kind: WindowKind,
    pub monitor: String,
    pub source: String,
}

/// Liveness of a WebView's web process. WebKit exposes it only as a signal,
/// so the handler flips a shared latch that the supervisor polls on its tick.
#[derive(Clone, Default)]
struct WebLiveness(Rc<Cell<bool>>);

impl WebLiveness {
    fn mark_terminated(&self) {
        self.0.set(true);
    }

    fn alive(&self) -> bool {
        !self.0.get()
    }
}

enum Content {
    Video {
        mpv: Rc<RefCell<Mpv>>,
    },
    Web {
        webview: webkit2gtk::WebView,
        /// Set by WebKit's `web-process-terminated` signal. A WebView has no
        /// other liveness API: without this latch a crashed web process leaves
        /// a blank window that the supervisor never replaces.
        liveness: WebLiveness,
    },
}

impl WallpaperWindow {
    pub fn video(
        app: &gtk::Application,
        monitor: &MonitorInfo,
        source: &str,
        volume: i64,
        muted: bool,
        profile: &str,
        hwdec: &str,
    ) -> anyhow::Result<Self> {
        let primary = monitor.is_primary;
        let (window, xid) = base_window(app, monitor)?;
        let mpv = match Mpv::spawn(&MpvOptions {
            wid: Some(xid),
            source: source.to_string(),
            audio_file: None,
            no_audio: !primary,
            volume: if primary { volume } else { 0 },
            muted,
            paused: true,
            loop_file: true,
            profile: profile.to_string(),
            hwdec: hwdec.to_string(),
        }) {
            Ok(mpv) => Rc::new(RefCell::new(mpv)),
            Err(err) => {
                // GTK adds the toplevel to the application and keeps its own
                // reference: dropping our wrapper alone would leave a blank
                // desktop window mapped forever.
                window.close();
                return Err(err);
            }
        };
        let menu = attach_menu(app, &window);
        Ok(Self {
            window,
            content: Content::Video { mpv },
            fade: Fade::default(),
            primary,
            muted,
            base_filter: crate::mpv::profile_video_filter(profile).to_string(),
            paused_at: None,
            parked: false,
            pause_timer: Fade::default(),
            park_allowed: false,
            key: WindowKey {
                kind: WindowKind::Video,
                monitor: monitor.name.clone(),
                source: source.to_string(),
            },
            _menu: menu,
        })
    }

    pub fn stream(
        app: &gtk::Application,
        monitor: &MonitorInfo,
        stream: &StreamInfo,
        volume: i64,
        muted: bool,
        profile: &str,
        hwdec: &str,
    ) -> anyhow::Result<Self> {
        let primary = monitor.is_primary;
        let (window, xid) = base_window(app, monitor)?;
        let mpv = match Mpv::spawn(&MpvOptions {
            wid: Some(xid),
            source: stream.video_url.clone(),
            audio_file: if primary {
                stream.audio_url.clone()
            } else {
                None
            },
            no_audio: !primary,
            volume: if primary { volume } else { 0 },
            muted,
            paused: true,
            loop_file: true,
            profile: profile.to_string(),
            hwdec: hwdec.to_string(),
        }) {
            Ok(mpv) => Rc::new(RefCell::new(mpv)),
            Err(err) => {
                window.close();
                return Err(err);
            }
        };
        let menu = attach_menu(app, &window);
        Ok(Self {
            window,
            content: Content::Video { mpv },
            fade: Fade::default(),
            primary,
            muted,
            base_filter: crate::mpv::profile_video_filter(profile).to_string(),
            paused_at: None,
            parked: false,
            pause_timer: Fade::default(),
            park_allowed: false,
            key: WindowKey {
                kind: WindowKind::Stream,
                monitor: monitor.name.clone(),
                source: stream.video_url.clone(),
            },
            _menu: menu,
        })
    }

    pub fn web(
        app: &gtk::Application,
        monitor: &MonitorInfo,
        uri: &str,
        muted: bool,
        allow_network: bool,
        tx: CommandSender,
    ) -> anyhow::Result<Self> {
        if !allow_network && !crate::util::is_local_uri(uri) {
            anyhow::bail!(
                "Network access is disabled for web wallpapers; choose a local HTML file"
            );
        }
        let primary = monitor.is_primary;
        let (window, _) = base_window(app, monitor)?;

        // Explicit security posture instead of inheriting every WebKit
        // default. Local pages may reference files next to them, but a file
        // page must not gain universal access to the filesystem, and pages
        // never get camera/microphone/notification permissions.
        let settings = webkit2gtk::Settings::new();
        settings.set_allow_file_access_from_file_urls(true);
        settings.set_allow_universal_access_from_file_urls(false);
        settings.set_enable_developer_extras(false);
        settings.set_enable_webgl(true);
        settings.set_enable_media(true);
        settings.set_enable_media_stream(true);
        settings.set_enable_mediasource(true);
        let webview = webkit2gtk::WebView::builder().settings(&settings).build();
        webview.set_is_muted(muted || !primary);
        webview.connect_create(|_, _| None);
        webview.connect_permission_request(|_, request| {
            request.deny();
            true
        });
        if !allow_network {
            webview.connect_decide_policy(|_, decision, decision_type| {
                if !matches!(
                    decision_type,
                    webkit2gtk::PolicyDecisionType::NavigationAction
                        | webkit2gtk::PolicyDecisionType::NewWindowAction
                ) {
                    return false;
                }
                let Some(navigation) =
                    decision.downcast_ref::<webkit2gtk::NavigationPolicyDecision>()
                else {
                    return false;
                };
                let Some(action) = navigation.navigation_action() else {
                    return false;
                };
                let Some(request) = action.request() else {
                    return false;
                };
                let Some(uri) = request.uri() else {
                    return false;
                };
                if crate::util::is_local_uri(uri.as_str()) {
                    return false;
                }
                decision.ignore();
                true
            });
        }

        // WebKit gives the page one automatic retry, then reports the failure
        // to the command loop where it becomes a user-visible message. Without
        // this a bad URL or a dropped network leaves a silent blank desktop.
        {
            let retried = Cell::new(false);
            let reported = Cell::new(false);
            let tx = tx.clone();
            let monitor = monitor.name.clone();
            webview.connect_load_failed(move |view, _event, uri, error| {
                log::warn!("[Web] {monitor}: {uri}: {error}");
                if !reported.replace(true) {
                    let _ = tx.send(Command::WebError {
                        monitor: monitor.clone(),
                        message: format!("{uri}: {error}"),
                    });
                }
                if !retried.replace(true) {
                    let view = view.clone();
                    glib::timeout_add_local_once(Duration::from_secs(2), move || view.reload());
                }
                false
            });
        }

        // A crashed web process cannot be revived in place; the supervisor
        // notices the latch on its next tick, drops the window and recreates
        // it through the same restart policy mpv uses.
        let liveness = WebLiveness::default();
        {
            let liveness = liveness.clone();
            let monitor = monitor.name.clone();
            webview.connect_web_process_terminated(move |_view, reason| {
                log::warn!("[Web] {monitor}: web process terminated: {:?}", reason);
                liveness.mark_terminated();
            });
        }

        window.add(&webview);
        webview.show();
        webview.load_uri(uri);

        let menu = attach_menu(app, &window);
        let menu_for_context = menu.clone();
        let window_for_context = window.clone();
        let _ = &window_for_context;
        webview.connect_context_menu(move |_, _, event, _| {
            menu_for_context.popup_at_pointer(Some(event));
            true
        });

        Ok(Self {
            window,
            content: Content::Web { webview, liveness },
            fade: Fade::default(),
            primary,
            muted,
            base_filter: String::new(),
            paused_at: None,
            parked: false,
            pause_timer: Fade::default(),
            park_allowed: false,
            key: WindowKey {
                kind: WindowKind::Web,
                monitor: monitor.name.clone(),
                source: uri.to_string(),
            },
            _menu: menu,
        })
    }

    pub fn key(&self) -> &WindowKey {
        &self.key
    }

    pub fn is_primary(&self) -> bool {
        self.primary
    }

    /// Point an existing video player at another file. Keeps the X11 window
    /// and the mpv process, which saves the ~120 ms cold start.
    pub fn load_video(&mut self, source: &str, volume: i64, muted: bool) -> anyhow::Result<()> {
        let Content::Video { mpv } = &self.content else {
            anyhow::bail!("window does not embed a player");
        };
        let mut mpv = mpv.borrow_mut();
        mpv.set_volume(if self.primary { volume } else { 0 })?;
        mpv.set_mute(muted)?;
        mpv.load_file(source)?;
        drop(mpv);
        // The new source starts from its own beginning; any state from a
        // parked decoder belongs to the old file.
        self.parked = false;
        self.paused_at = None;
        self.key.source = source.to_string();
        Ok(())
    }

    /// Last command-level mpv error (for example a corrupt file passed to
    /// `loadfile`). The command loop polls this and shows it to the user.
    pub fn take_error(&self) -> Option<String> {
        match &self.content {
            Content::Video { mpv } => mpv.borrow().take_error(),
            Content::Web { .. } => None,
        }
    }

    /// True while the embedded player is alive. Web views report alive here;
    /// their load failures are surfaced separately by WebKit signals.
    pub fn player_alive(&mut self) -> bool {
        match &mut self.content {
            Content::Video { mpv } => mpv.borrow_mut().is_running(),
            Content::Web { liveness, .. } => liveness.alive(),
        }
    }

    pub fn play_fade(&mut self, target: i64, duration: f64, interval: f64) {
        self.paused_at = None;
        self.pause_timer.cancel();
        self.unpark(false);
        match &self.content {
            Content::Video { mpv } => {
                let _ = mpv.borrow_mut().set_pause(false);
                if self.primary {
                    let mpv = Rc::clone(mpv);
                    self.fade.start(
                        0.0,
                        target as f64,
                        duration,
                        interval,
                        move |volume| {
                            let _ = mpv.borrow_mut().set_volume(volume as i64);
                        },
                        || {},
                    );
                }
            }
            Content::Web { webview, .. } => {
                // WebKitGTK has no public freeze API; muting is the part of
                // "pause" we can guarantee. WebKit already throttles rendering
                // for an unmapped/background surface.
                webview.set_is_muted(self.muted || !self.primary);
            }
        }
    }

    /// Reload a parked file at the position captured by the worker. `paused`
    /// keeps the restored frame on screen without starting playback, which is
    /// what a monitor returning while Eco is still idle needs.
    fn unpark(&mut self, paused: bool) {
        if !std::mem::take(&mut self.parked) {
            return;
        }
        let Content::Video { mpv } = &self.content else {
            return;
        };
        if let Err(err) = mpv.borrow_mut().unpark(&self.key.source) {
            log::warn!("[Eco] could not unpark {}: {err}", self.key.monitor);
            return;
        }
        let _ = mpv.borrow_mut().set_pause(paused);
    }

    /// Tell the window whether its monitor is currently invisible. Switching
    /// this off while parked reloads the file immediately so a monitor that
    /// becomes visible does not show an empty surface.
    pub fn set_park_allowed(&mut self, allowed: bool) {
        if needs_unpark(self.parked, allowed) {
            self.unpark(true);
        }
        self.park_allowed = allowed;
    }

    pub fn pause_fade(&mut self, duration: f64, interval: f64) {
        if self.paused_at.is_none() {
            self.paused_at = Some(Instant::now());
        }
        match &self.content {
            Content::Video { mpv } => {
                if self.primary {
                    let from = mpv.borrow().volume() as f64;
                    let mpv_for_volume = Rc::clone(mpv);
                    self.fade.start(
                        from,
                        0.0,
                        duration,
                        interval,
                        move |volume| {
                            let _ = mpv_for_volume.borrow_mut().set_volume(volume as i64);
                        },
                        || {},
                    );
                    // The pause must not ride on the volume fade: the mute rule
                    // and the volume slider start fades on the same slot, and
                    // cancelling one used to cancel the pause with it.
                    let mpv_for_pause = Rc::clone(mpv);
                    self.pause_timer.finish_after(duration, interval, move || {
                        let _ = mpv_for_pause.borrow_mut().set_pause(true);
                    });
                } else {
                    let _ = mpv.borrow_mut().set_pause(true);
                }
            }
            Content::Web { webview, .. } => webview.set_is_muted(true),
        }
    }

    /// Release the decoder after a long pause, but only while the monitor is
    /// invisible. This is deliberately restricted to local video windows:
    /// streams have an extra audio track and expiring URLs, and a black frame
    /// on a visible monitor is never acceptable. Returns true when a park
    /// request was sent.
    pub fn park_if_due(&mut self, now: Instant, threshold: Duration) -> bool {
        if self.parked || !self.park_allowed || self.key.kind != WindowKind::Video {
            return false;
        }
        if !crate::eco::should_park(self.paused_at, now, threshold) {
            return false;
        }
        let Content::Video { mpv } = &self.content else {
            return false;
        };
        if let Err(err) = mpv.borrow_mut().park() {
            log::warn!(
                "[Eco] could not park the decoder on {}: {err}",
                self.key.monitor
            );
            return false;
        }
        self.parked = true;
        log::debug!(
            "[Eco] parked the decoder on {} ({})",
            self.key.monitor,
            self.key.source
        );
        true
    }

    pub fn volume_fade(&mut self, target: i64, duration: f64, interval: f64) {
        if let Content::Video { mpv } = &self.content {
            let from = mpv.borrow().volume() as f64;
            let mpv = Rc::clone(mpv);
            self.fade.start(
                from,
                target as f64,
                duration,
                interval,
                move |volume| {
                    let _ = mpv.borrow_mut().set_volume(volume as i64);
                },
                || {},
            );
        }
    }

    pub fn set_volume(&mut self, volume: i64) {
        if let Content::Video { mpv } = &self.content {
            let _ = mpv.borrow_mut().set_volume(volume);
        }
    }

    /// Apply (or clear) the Eco Engine's idle frame-rate cap. `None` restores the
    /// profile's own filter chain, so Eco's 30 fps cap is not lost after a resume.
    pub fn set_fps_cap(&mut self, fps: Option<u32>) {
        if let Content::Video { mpv } = &self.content {
            let filter = match fps {
                Some(fps) => format!("fps={fps}"),
                None => self.base_filter.clone(),
            };
            let _ = mpv.borrow_mut().set_video_filter(&filter);
        }
    }

    pub fn set_mute(&mut self, muted: bool) {
        self.muted = muted;
        match &self.content {
            Content::Video { mpv } => {
                let _ = mpv.borrow_mut().set_mute(muted);
            }
            Content::Web { webview, .. } => webview.set_is_muted(muted || !self.primary),
        }
    }
}

impl Drop for WallpaperWindow {
    fn drop(&mut self) {
        self.fade.cancel();
        self.pause_timer.cancel();
        self.window.close();
    }
}

/// A parked player must be reloaded as soon as its monitor stops qualifying
/// for parking, even when the Eco plan keeps it paused (idle Eco after unlock).
fn needs_unpark(parked: bool, allowed: bool) -> bool {
    parked && !allowed
}

fn base_window(
    app: &gtk::Application,
    monitor: &MonitorInfo,
) -> anyhow::Result<(gtk::ApplicationWindow, u64)> {
    let window = gtk::ApplicationWindow::new(app);
    window.set_decorated(false);
    window.set_resizable(false);
    window.set_type_hint(gdk::WindowTypeHint::Desktop);
    window.set_skip_taskbar_hint(true);
    window.set_skip_pager_hint(true);
    window.set_accept_focus(false);
    window.set_can_focus(false);
    window.set_default_size(monitor.width, monitor.height);
    window.set_size_request(monitor.width, monitor.height);
    window.move_(monitor.x, monitor.y);
    window.show_all();
    window.resize(monitor.width, monitor.height);

    let gdk_window = window
        .window()
        .ok_or_else(|| anyhow::anyhow!("wallpaper window was not realized"))?;
    let x11 = gdk_window
        .downcast::<gdkx11::X11Window>()
        .map_err(|_| anyhow::anyhow!("GDK is not using X11; cannot embed the video surface"))?;
    let xid = x11.xid();

    // Desktop-icons extensions keep their own window at the bottom of the
    // DESKTOP layer. A freshly mapped wallpaper window sits above it and hides
    // the icons, so put ourselves below it now and again after the icons
    // extension has settled.
    lower_x11_window(xid);
    let _ = glib::timeout_add_local(Duration::from_millis(2500), move || {
        lower_x11_window(xid);
        glib::ControlFlow::Break
    });

    Ok((window, xid))
}

fn attach_menu(app: &gtk::Application, window: &gtk::ApplicationWindow) -> gtk::Menu {
    let menu = menu::wallpaper_menu(app);
    window.add_events(gdk::EventMask::BUTTON_PRESS_MASK);
    let popup = menu.clone();
    window.connect_button_press_event(move |_, event| {
        if event.button() == 3 {
            popup.popup_at_pointer(Some(event));
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    menu
}

/// Put an X11 window below other windows in the same layer. Used to keep the
/// wallpaper underneath the desktop icons.
fn lower_x11_window(xid: u64) {
    let Ok((connection, _)) = x11rb::connect(None) else {
        return;
    };
    use x11rb::protocol::xproto::{ConfigureWindowAux, StackMode};
    let _ = connection.configure_window(
        xid as u32,
        &ConfigureWindowAux::new().stack_mode(StackMode::BELOW),
    );
    let _ = connection.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(kind: WindowKind, monitor: &str, source: &str) -> WindowKey {
        WindowKey {
            kind,
            monitor: monitor.to_string(),
            source: source.to_string(),
        }
    }

    #[test]
    fn parking_permission_revocation_requires_a_reload_only_while_parked() {
        assert!(!needs_unpark(false, false));
        assert!(!needs_unpark(false, true));
        assert!(!needs_unpark(true, true));
        assert!(needs_unpark(true, false));
    }

    #[test]
    fn keys_match_only_when_all_fields_do() {
        let original = key(WindowKind::Video, "eDP-1", "/videos/a.mp4");
        assert!(original == original.clone());
        // Same monitor and file, different mode: a new window is needed.
        assert_ne!(original, key(WindowKind::Stream, "eDP-1", "/videos/a.mp4"));
        // Another monitor or another file: rebuild only that window.
        assert_ne!(original, key(WindowKind::Video, "HDMI-1", "/videos/a.mp4"));
        assert_ne!(original, key(WindowKind::Video, "eDP-1", "/videos/b.mp4"));
    }

    #[test]
    fn a_terminated_web_process_is_dead_for_every_holder() {
        // The signal handler and the supervisor hold clones of the same latch;
        // if each had its own Cell the crash would never reach the poll.
        let liveness = WebLiveness::default();
        let polled = liveness.clone();
        assert!(polled.alive());
        liveness.mark_terminated();
        assert!(!polled.alive());
        // Termination is one-way: the window is replaced, never reused.
        liveness.mark_terminated();
        assert!(!polled.alive());
    }

    #[test]
    fn unchanged_monitors_survive_a_sync_plan() {
        // Mirrors App::sync_windows: a window is kept when its key is still
        // in the desired set, so only the changed monitor is rebuilt.
        let desired = [
            key(WindowKind::Video, "eDP-1", "/videos/a.mp4"),
            key(WindowKind::Video, "HDMI-1", "/videos/new.mp4"),
        ];
        let existing = [
            key(WindowKind::Video, "eDP-1", "/videos/a.mp4"),
            key(WindowKind::Video, "HDMI-1", "/videos/old.mp4"),
        ];
        let kept: Vec<&WindowKey> = existing
            .iter()
            .filter(|window| desired.contains(window))
            .collect();
        assert_eq!(kept, vec![&existing[0]]);
    }
}
