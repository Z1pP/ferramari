//! GTK-free wallpaper playback engine.
//!
//! Owns a native surface plus its player, and exposes the control surface the
//! application loop needs: play/pause with fade, mute, volume, fps cap,
//! decoder parking, restart-on-death. The GTK `ui/wallpaper.rs` had this same
//! shape but wrapped a `gtk::ApplicationWindow`; the window is now an opaque
//! platform-provided surface and the player is the same mpv IPC child process.
//!
//! What is *not* here (yet):
//! * the web wallpaper content — WebKitGTK was the only engine with a
//!   maintained GTK-3 binding, so web pages are handled in the web engine
//!   phase (docs/archive/GTK_REMOVAL_AUDIT.md §4). `Engine::web` returns a clear
//!   error instead of silently doing the wrong thing.
//! * the right-click context menu on the wallpaper — a GTK popup; the native
//!   replacement (X11 popup or Iced overlay) belongs to the system
//!   integration phase.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ferramari_core::eco;
use ferramari_core::hwdec::HwDec;
use ferramari_core::platform::{WallpaperSurface, WallpaperSurfaceProvider};
use ferramari_core::MonitorInfo;

use crate::fade::Fade;
use crate::mpv::{self, Mpv, Options as MpvOptions};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContentKind {
    Video,
    Stream,
    Web,
}

/// What a wallpaper window shows: kind, monitor and source. Two windows are
/// interchangeable only when all three match, which lets a change on one
/// monitor keep the other players running.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WindowKey {
    pub kind: ContentKind,
    pub monitor: String,
    pub source: String,
}

enum Content {
    Video { mpv: Arc<Mutex<Mpv>> },
}

/// A wallpaper surface with its media attached. Dropping stops the fade
/// threads, kills the player and destroys the X window.
pub struct Engine {
    surface: Box<dyn WallpaperSurface>,
    content: Content,
    fade: Fade,
    key: WindowKey,
    primary: bool,
    muted: bool,
    base_filter: String,
    paused_at: Option<Instant>,
    parked: bool,
    pause_timer: Fade,
    park_allowed: bool,
}

impl Engine {
    /// A window that renders a local video on `monitor` via mpv.
    pub fn video(
        surface_provider: &dyn WallpaperSurfaceProvider,
        monitor: &MonitorInfo,
        source: &str,
        volume: i64,
        muted: bool,
        profile: &str,
        hwdec: &str,
    ) -> anyhow::Result<Self> {
        let primary = monitor.is_primary;
        let surface = surface_provider.create(monitor)?;
        let mpv = match Mpv::spawn(&MpvOptions {
            wid: Some(surface.embedding_handle()),
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
            Ok(mpv) => Arc::new(Mutex::new(mpv)),
            Err(err) => {
                // Drop the surface before returning: an abandoned window
                // would sit on the desktop forever with nothing in it.
                drop(surface);
                return Err(err);
            }
        };
        Ok(Self {
            surface,
            content: Content::Video { mpv },
            fade: Fade::default(),
            key: WindowKey {
                kind: ContentKind::Video,
                monitor: monitor.name.clone(),
                source: source.to_string(),
            },
            primary,
            muted,
            base_filter: mpv::profile_video_filter(profile).to_string(),
            paused_at: None,
            parked: false,
            pause_timer: Fade::default(),
            park_allowed: false,
        })
    }

    /// A window that renders a yt-dlp resolved stream on `monitor`.
    pub fn stream(
        surface_provider: &dyn WallpaperSurfaceProvider,
        monitor: &MonitorInfo,
        stream: &ferramari_core::ytdlp::StreamInfo,
        volume: i64,
        muted: bool,
        profile: &str,
        hwdec: &str,
    ) -> anyhow::Result<Self> {
        let primary = monitor.is_primary;
        let surface = surface_provider.create(monitor)?;
        let mpv = match Mpv::spawn(&MpvOptions {
            wid: Some(surface.embedding_handle()),
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
            Ok(mpv) => Arc::new(Mutex::new(mpv)),
            Err(err) => {
                drop(surface);
                return Err(err);
            }
        };
        Ok(Self {
            surface,
            content: Content::Video { mpv },
            fade: Fade::default(),
            key: WindowKey {
                kind: ContentKind::Stream,
                monitor: monitor.name.clone(),
                source: stream.video_url.clone(),
            },
            primary,
            muted,
            base_filter: mpv::profile_video_filter(profile).to_string(),
            paused_at: None,
            parked: false,
            pause_timer: Fade::default(),
            park_allowed: false,
        })
    }

    /// Web wallpapers are not available yet. See the module docs.
    pub fn web(monitor: &MonitorInfo, _uri: &str) -> anyhow::Result<Self> {
        anyhow::bail!(
            "web wallpapers on {}: the GTK-free web engine is not ready yet \
             (see docs/archive/GTK_REMOVAL_AUDIT.md, Phase 5)",
            monitor.name
        )
    }

    pub fn key(&self) -> &WindowKey {
        &self.key
    }

    pub fn is_primary(&self) -> bool {
        self.primary
    }

    /// The opaque native handle the player renders into. It is valid only
    /// while this engine owns the platform surface and must never be stored.
    pub fn surface_handle(&self) -> u64 {
        self.surface.embedding_handle()
    }

    /// Point an existing video player at another file. Keeps the native surface
    /// and the mpv process, which saves the ~120 ms cold start.
    pub fn load_video(&mut self, source: &str, volume: i64, muted: bool) -> anyhow::Result<()> {
        let Content::Video { mpv } = &self.content;
        let mut mpv = mpv
            .lock()
            .map_err(|_| anyhow::anyhow!("mpv lock poisoned"))?;
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
            Content::Video { mpv } => mpv.lock().ok().and_then(|mpv| mpv.take_error()),
        }
    }

    /// Ask the player which decoder it is actually using and report the last
    /// known answer. The request is queued on the IPC worker, so the first
    /// call returns [`HwDec::Unknown`] and a later supervision tick sees the
    /// reply — the command loop never waits on mpv.
    ///
    /// This exists because the setting is not the fact: mpv accepts
    /// `--hwdec=vaapi` on a machine without a VA-API path and decodes on the
    /// CPU without failing. Comparing the two is what turns a silent fallback
    /// into something the user can see.
    pub fn hwdec_status(&mut self) -> HwDec {
        let Content::Video { mpv } = &self.content;
        let Ok(mut mpv) = mpv.lock() else {
            return HwDec::Unknown;
        };
        let current = mpv.hwdec_current();
        if current == HwDec::Unknown {
            // Nothing recorded yet; ask once per call until an answer lands.
            let _ = mpv.probe_hwdec();
        }
        current
    }

    /// What the decoder reported at the previous probe. Reading it also asks
    /// the worker for a fresh round, so one supervision tick per second keeps
    /// the Performance page current without ever blocking on mpv.
    pub fn playback_stats(&mut self) -> mpv::PlaybackStats {
        let Content::Video { mpv } = &self.content;
        let Ok(mut mpv) = mpv.lock() else {
            return mpv::PlaybackStats::default();
        };
        let stats = mpv.stats();
        let _ = mpv.probe_hwdec();
        stats
    }

    /// True while the embedded player is alive.
    pub fn player_alive(&mut self) -> bool {
        match &mut self.content {
            Content::Video { mpv } => {
                let Ok(mut mpv) = mpv.lock() else {
                    return false;
                };
                mpv.is_running()
            }
        }
    }

    pub fn play_fade(&mut self, target: i64, duration: f64, interval: f64) {
        self.paused_at = None;
        self.pause_timer.cancel();
        self.unpark(false);
        let Content::Video { mpv } = &self.content;
        {
            let Ok(mut mpv) = mpv.lock() else {
                return;
            };
            let _ = mpv.set_pause(false);
        }
        if !self.primary {
            return;
        }
        let mpv = Arc::clone(mpv);
        self.fade.start(
            0.0,
            target as f64,
            duration,
            interval,
            move |volume| {
                if let Ok(mut mpv) = mpv.lock() {
                    let _ = mpv.set_volume(volume as i64);
                }
            },
            || {},
        );
    }

    /// Reload a parked file at the position captured by the worker. `paused`
    /// keeps the restored frame on screen without starting playback, which is
    /// what a monitor returning while Eco is still idle needs.
    fn unpark(&mut self, paused: bool) {
        if !std::mem::take(&mut self.parked) {
            return;
        }
        let Content::Video { mpv } = &self.content;
        let Ok(mut mpv) = mpv.lock() else {
            return;
        };
        if let Err(err) = mpv.unpark(&self.key.source) {
            log::warn!("[Eco] could not unpark {}: {err}", self.key.monitor);
            return;
        }
        let _ = mpv.set_pause(paused);
    }

    /// Tell the engine whether its monitor is currently invisible. Switching
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
        let Content::Video { mpv } = &self.content;
        if !self.primary {
            let Ok(mut mpv) = mpv.lock() else {
                return;
            };
            let _ = mpv.set_pause(true);
            return;
        }
        let from = {
            let Ok(mpv) = mpv.lock() else {
                return;
            };
            mpv.volume() as f64
        };
        let mpv_for_volume = Arc::clone(mpv);
        self.fade.start(
            from,
            0.0,
            duration,
            interval,
            move |volume| {
                if let Ok(mut mpv) = mpv_for_volume.lock() {
                    let _ = mpv.set_volume(volume as i64);
                }
            },
            || {},
        );
        // The pause must not ride on the volume fade: the mute rule and the
        // volume slider start fades on the same slot, and cancelling one used
        // to cancel the pause with it.
        let mpv_for_pause = Arc::clone(mpv);
        self.pause_timer.finish_after(duration, interval, move || {
            if let Ok(mut mpv) = mpv_for_pause.lock() {
                let _ = mpv.set_pause(true);
            }
        });
    }

    /// Release the decoder after a long pause, but only while the monitor is
    /// invisible. This is deliberately restricted to local video engines:
    /// streams have an extra audio track and expiring URLs, and a black frame
    /// on a visible monitor is never acceptable. Returns true when a park
    /// request was sent.
    pub fn park_if_due(&mut self, now: Instant, threshold: Duration) -> bool {
        if self.parked || !self.park_allowed || self.key.kind != ContentKind::Video {
            return false;
        }
        if !eco::should_park(self.paused_at, now, threshold) {
            return false;
        }
        let Content::Video { mpv } = &self.content;
        let Ok(mut mpv) = mpv.lock() else {
            return false;
        };
        if let Err(err) = mpv.park() {
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
        let Content::Video { mpv } = &self.content;
        let Ok(guard) = mpv.lock() else {
            return;
        };
        let from = guard.volume() as f64;
        drop(guard);
        let mpv = Arc::clone(mpv);
        self.fade.start(
            from,
            target as f64,
            duration,
            interval,
            move |volume| {
                if let Ok(mut mpv) = mpv.lock() {
                    let _ = mpv.set_volume(volume as i64);
                }
            },
            || {},
        );
    }

    pub fn set_volume(&mut self, volume: i64) {
        let Content::Video { mpv } = &self.content;
        if let Ok(mut mpv) = mpv.lock() {
            let _ = mpv.set_volume(volume);
        }
    }

    /// Apply (or clear) the Eco Engine's idle frame-rate cap. `None` restores
    /// the profile's own filter chain, so Eco's 30 fps cap is not lost after a
    /// resume.
    pub fn set_fps_cap(&mut self, fps: Option<u32>) {
        let Content::Video { mpv } = &self.content;
        let filter = match fps {
            Some(fps) => format!("fps={fps}"),
            None => self.base_filter.clone(),
        };
        if let Ok(mut mpv) = mpv.lock() {
            let _ = mpv.set_video_filter(&filter);
        }
    }

    pub fn set_mute(&mut self, muted: bool) {
        self.muted = muted;
        let Content::Video { mpv } = &self.content;
        if let Ok(mut mpv) = mpv.lock() {
            let _ = mpv.set_mute(muted);
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        // Stop the fade threads first: their callbacks reference the player.
        self.fade.cancel();
        self.pause_timer.cancel();
        // Content drops next (kills mpv), then the surface (unmaps the window).
    }
}

/// A parked player must be reloaded as soon as its monitor stops qualifying
/// for parking, even when the Eco plan keeps it paused (idle Eco after unlock).
fn needs_unpark(parked: bool, allowed: bool) -> bool {
    parked && !allowed
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferramari_core::DisplayIdentity;

    fn key(kind: ContentKind, monitor: &str, source: &str) -> WindowKey {
        WindowKey {
            kind,
            monitor: monitor.to_string(),
            source: source.to_string(),
        }
    }

    #[test]
    fn keys_match_only_when_all_fields_do() {
        let original = key(ContentKind::Video, "eDP-1", "/videos/a.mp4");
        assert_eq!(original, original.clone());
        // Same monitor and file, different mode: a new window is needed.
        assert_ne!(original, key(ContentKind::Stream, "eDP-1", "/videos/a.mp4"));
        // Another monitor or another file: rebuild only that window.
        assert_ne!(original, key(ContentKind::Video, "HDMI-1", "/videos/a.mp4"));
        assert_ne!(original, key(ContentKind::Video, "eDP-1", "/videos/b.mp4"));
    }

    #[test]
    fn parking_permission_revocation_requires_a_reload_only_while_parked() {
        assert!(!needs_unpark(false, false));
        assert!(!needs_unpark(false, true));
        assert!(!needs_unpark(true, true));
        assert!(needs_unpark(true, false));
    }

    #[test]
    fn unchanged_monitors_survive_a_sync_plan() {
        // Mirrors App::sync_windows: a window is kept when its key is still
        // in the desired set, so only the changed monitor is rebuilt.
        let desired = [
            key(ContentKind::Video, "eDP-1", "/videos/a.mp4"),
            key(ContentKind::Video, "HDMI-1", "/videos/new.mp4"),
        ];
        let existing = [
            key(ContentKind::Video, "eDP-1", "/videos/a.mp4"),
            key(ContentKind::Video, "HDMI-1", "/videos/old.mp4"),
        ];
        let kept: Vec<&WindowKey> = existing
            .iter()
            .filter(|window| desired.contains(window))
            .collect();
        assert_eq!(kept, vec![&existing[0]]);
    }

    #[test]
    fn web_content_reports_the_missing_engine_explicitly() {
        let monitor = MonitorInfo {
            index: 0,
            name: "eDP-1".into(),
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
            scale_factor: 1,
            refresh_hz: None,
            is_primary: true,
            identity: DisplayIdentity::default(),
        };
        let error = Engine::web(&monitor, "file:///tmp/a.html")
            .err()
            .expect("web must not pretend to work");
        assert!(error.to_string().contains("web engine is not ready"));
    }
}
